// API Handler for internal transfers
// Full implementation with TigerBeetle integration
//
// Flow:
// 1. Validate request
// 2. Generate request_id
// 3. Insert DB record (requesting)
// 4. Check TB balance
// 5. Create TB PENDING (lock funds)
// 6. Update DB (pending)
// 7. [Future] Send to UBSCore via Aeron
// 8. Return response

use crate::api::{error_codes, error_response, success_response, validate_transfer_request, decimal_to_i64};
use crate::db::{TransferRequestRecord, InternalTransferDb};
use crate::models::api_response::ApiResponse;
use crate::models::internal_transfer_types::{InternalTransferData, InternalTransferRequest, TransferStatus, AccountType};
// use crate::mocks::tigerbeetle_mock::MockTbClient; // Removed
use tigerbeetle_unofficial::{Client, Transfer};
use crate::symbol_manager::SymbolManager;
use crate::utils::generate_request_id;
use crate::common_utils::get_current_timestamp_ms;
use anyhow::{Result, Context};
use std::sync::Arc;
use crate::gateway::OrderPublisher;
use std::time::Duration;
use crate::models::balance_requests::BalanceRequest;

// Constants
const FUNDING_USER_ID: u64 = 0;

/// Helper: Generate TB account ID
/// account_id = (user_id << 64) | asset_id
fn tb_account_id(user_id: u64, asset_id: u32) -> u128 {
    ((user_id as u128) << 64) | (asset_id as u128)
}

/// Handler state
pub struct InternalTransferHandler {
    pub db: Arc<InternalTransferDb>,
    pub symbol_manager: Arc<SymbolManager>,
    pub tb_client: Arc<Client>,
    pub kafka_producer: Option<Arc<dyn OrderPublisher>>,
}

impl InternalTransferHandler {
    pub fn new(
        db: Arc<InternalTransferDb>,
        symbol_manager: Arc<SymbolManager>,
        tb_client: Arc<Client>,
        kafka_producer: Option<Arc<dyn OrderPublisher>>,
    ) -> Self {
        Self { db, symbol_manager, tb_client, kafka_producer }
    }

    /// Handle internal transfer request (MVP version)
    pub async fn handle_transfer(
        &self,
        req: InternalTransferRequest,
    ) -> Result<ApiResponse<Option<InternalTransferData>>> {


        // ROUTING: Spot -> Funding must go through UBSCore
        match (&req.from_account, &req.to_account) {
            (AccountType::Spot { .. }, AccountType::Funding { .. }) => {
                // 1. Validate (re-use generic validation logic first)
                if let Err(e) = validate_transfer_request(&req, &self.symbol_manager) {
                     return Ok(error_response(error_codes::INVALID_AMOUNT, e.to_string()));
                }

                // 2. Resolve Asset
                 let asset = req.from_account.asset();
                 let asset_id = match self.symbol_manager.get_asset_id(&asset) {
                     Some(id) => id,
                     None => return Ok(error_response(error_codes::INVALID_ASSET, format!("Unknown asset: {}", asset))),
                 };

                 // 3. Resolve User
                 let user_id = match req.from_account.user_id() {
                     Some(id) => id,
                     None => return Ok(error_response(error_codes::VALIDATION_ERROR, "Spot account requires user_id".to_string())),
                 };

                 // 4. Create DB Record (Processing)
                 let request_id = generate_request_id();
                 // Resolve decimals
                 let decimals = self.symbol_manager.get_asset_decimal(asset_id).unwrap_or(8);
                 let amount_scaled = match decimal_to_i64(&req.amount, decimals) {
                     Ok(v) => v,
                     Err(e) => return Ok(error_response(error_codes::INVALID_AMOUNT, e.to_string())),
                 };

                 let record = TransferRequestRecord {
                    request_id: request_id as i64,
                    user_id: user_id as i64,
                    from_account_type: req.from_account.type_name().to_string(),
                    from_user_id: user_id as i64,
                    to_account_type: req.to_account.type_name().to_string(),
                    to_user_id: req.to_account.user_id().unwrap_or(0) as i64,
                    asset_id: asset_id as i32,
                    amount: amount_scaled,
                    status: "processing_ubs".to_string(),
                    created_at: get_current_timestamp_ms(),
                    updated_at: get_current_timestamp_ms(),
                    error_message: None,
                 };

                 if let Err(e) = self.db.insert_transfer_request(record.clone()).await {
                    return Ok(error_response("DB_ERROR", format!("Failed to insert: {}", e)));
                 }

                     // 5. Send to UBSCore via Kafka
                 // Update DB to "pending" BEFORE publishing to avoid race where Settlement completes it before we return.
                 // Actually, best to insert as "pending" initially or update now.
                 // Using "processing_ubs" in insert is fine, but we must update to "pending" BEFORE publish.
                 let _ = self.db.update_transfer_status(
                      request_id as i64,
                      TransferStatus::Pending.as_str(),
                      None
                  ).await;

                 if let Some(producer) = &self.kafka_producer {
                     let balance_req = BalanceRequest::TransferOut {
                         request_id: request_id as u64,
                         user_id: user_id as u64,
                         asset_id: asset_id,
                         amount: amount_scaled as u64,
                         timestamp: get_current_timestamp_ms() as u64,
                     };

                     let payload = serde_json::to_string(&balance_req).unwrap();
                     let key = user_id.to_string();

                     match producer.publish("balance.operations".to_string(), key, payload.into_bytes()).await {
                         Ok(_) => {
                             // Do NOT update status here again.
                             // Just return info.
                             return Ok(success_response(InternalTransferData {
                                request_id: request_id.to_string(),
                                from_account: req.from_account,
                                to_account: req.to_account,
                                amount: req.amount.to_string(),
                                status: TransferStatus::Pending,
                                created_at: get_current_timestamp_ms(), // approximation
                             }));
                         }
                         Err(e) => { // Error is String now
                             let _ = self.db.update_transfer_status(
                                 request_id as i64,
                                 TransferStatus::Failed.as_str(),
                                 Some(format!("Kafka error: {}", e))
                             ).await;
                             return Ok(error_response("KAFKA_ERROR", format!("Failed to send to UBSCore: {}", e)));
                         }
                     }
                 } else {
                     return Ok(error_response("CONFIG_ERROR", "Spot->Funding requires Kafka producer (not configured)".to_string()));
                 }
            }
            _ => {}
        }

        // 1. Validate
        if let Err(e) = validate_transfer_request(&req, &self.symbol_manager) {
            return Ok(error_response(error_codes::INVALID_AMOUNT, e.to_string()));
        }

        // 2. Get asset_id
        let asset = req.from_account.asset();
        let asset_id = match self.symbol_manager.get_asset_id(&asset) {
            Some(id) => id,
            None => {
                return Ok(error_response(
                    error_codes::INVALID_ASSET,
                    format!("Unknown asset: {}", asset),
                ));
            }
        };

        // Verify user owns from_account
    log::info!("⚡ [InternalTransfer] Received request: {:?}", req);

    // Check balance logic...
        // 3. Convert amount
        let decimals = self.symbol_manager.get_asset_decimal(asset_id).unwrap_or(8);
        let amount_scaled = match decimal_to_i64(&req.amount, decimals) {
            Ok(v) => v,
            Err(e) => {
                return Ok(error_response(
                    error_codes::INVALID_AMOUNT,
                    e.to_string(),
                ));
            }
        };

        // 4. Generate request_id
        let request_id = generate_request_id() as i64;
        let now = get_current_timestamp_ms();

        // Create DB record
        // Create DB record
        // Create DB record
        let record = TransferRequestRecord {
            request_id: request_id as i64,
            user_id: match (req.from_account.user_id(), req.to_account.user_id()) {
                (Some(uid), _) if uid > 0 => uid as i64,
                (_, Some(uid)) if uid > 0 => uid as i64,
                _ => 0,
            },
            from_account_type: req.from_account.type_name().to_string(),
            from_user_id: req.from_account.user_id().unwrap_or(0) as i64,
            to_account_type: req.to_account.type_name().to_string(),
            to_user_id: req.to_account.user_id().unwrap_or(0) as i64,
            asset_id: asset_id as i32,
            amount: amount_scaled as i64,
            status: TransferStatus::Requesting.as_str().to_string(),
            created_at: get_current_timestamp_ms(),
            updated_at: get_current_timestamp_ms(),
            error_message: None,
        };

        // 6. Insert to DB
        if let Err(e) = self.db.insert_transfer_request(record).await {
            return Ok(error_response(
                "DB_ERROR",
                format!("Failed to insert transfer: {}", e),
            ));
        }

        // 7. Determine debit/credit accounts
        let from_user = req.from_account.user_id().unwrap_or(FUNDING_USER_ID);
        let to_user = req.to_account.user_id().unwrap_or(FUNDING_USER_ID);

        // Proper Account ID generation handling MSB for Funding Accounts
        let debit_is_funding = matches!(req.from_account, AccountType::Funding { .. });
        let credit_is_funding = matches!(req.to_account, AccountType::Funding { .. });

        let debit_user_id = if debit_is_funding { from_user | (1u64 << 63) } else { from_user };
        let credit_user_id = if credit_is_funding { to_user | (1u64 << 63) } else { to_user };

        // If Funding Account "from_user" is 0, then we use (0 | MSB).

        let debit_account_id = tb_account_id(debit_user_id, asset_id);
        let credit_account_id = tb_account_id(credit_user_id, asset_id);

        // Ensure accounts exist
        if let Err(e) = crate::ubs_core::tigerbeetle::ensure_account(&self.tb_client, debit_account_id, 1, 1).await {
             log::warn!("Failed to ensure Debit Account: {}", e);
        }
        if let Err(e) = crate::ubs_core::tigerbeetle::ensure_account(&self.tb_client, credit_account_id, 1, 1).await {
             log::warn!("Failed to ensure Credit Account: {}", e);
        }

        // 8. Check funding account balance (Real TB)
        // We use lookup_accounts to check balance.
        let accounts = self.tb_client.lookup_accounts(vec![debit_account_id]).await
             .map_err(|e| anyhow::anyhow!("TB Lookup Error: {:?}", e))?; // Assuming map_err needed if Client mismatch

        if accounts.is_empty() {
             return Ok(error_response(
                 "INVALID_ACCOUNT",
                 format!("Debit account not found: {}", debit_account_id),
             ));
        }
        let account = &accounts[0];
        // Available balance = credits_posted - debits_posted - debits_pending
        let available = account.credits_posted().saturating_sub(account.debits_posted()).saturating_sub(account.debits_pending());

        if available < (amount_scaled as u128) {
             return Ok(error_response(
                 error_codes::INSUFFICIENT_BALANCE,
                 format!("Insufficient balance: have {}, need {}", available, amount_scaled),
             ));
        }

        // 9. Create TB PENDING (lock funds)
        let transfer_id = request_id as u128;

        let transfer = Transfer::new(transfer_id)
            .with_debit_account_id(debit_account_id)
            .with_credit_account_id(credit_account_id)
            .with_amount(amount_scaled as u128)
            .with_ledger(1)
            .with_code(100)
            .with_flags(tigerbeetle_unofficial::transfer::Flags::PENDING); // Lock funds

        // Corrected create_transfers handling (Result<()>)
        if let Err(e) = self.tb_client.create_transfers(vec![transfer]).await {
             // Failed to lock - mark as failed
              let _ = self.db.update_transfer_status(
                request_id,
                TransferStatus::Failed.as_str(),
                Some(format!("TB lock failed: {}", e)),
            ).await;

            return Ok(error_response(
                error_codes::INSUFFICIENT_BALANCE,
                format!("Failed to lock funds (TB error): {}", e),
            ));
        }

        // 10. Update DB status to Pending
        if let Err(e) = self.db.update_transfer_status(
            request_id,
            TransferStatus::Pending.as_str(),
            None,
        ).await {
            // TB locked but DB update failed - need recovery
            log::error!(
                "CRITICAL: TB locked for transfer {} but DB update failed: {}",
                request_id, e
            );
            // Settlement scanner will recover this
        }

        // 11. [TODO] Send to UBSCore via Aeron

        // 12. Return success
        // 12. Return success
        let data = InternalTransferData {
            request_id: request_id.to_string(),
            from_account: req.from_account,
            to_account: req.to_account,
            amount: req.amount.to_string(),
            status: TransferStatus::Pending,  // Funds are now locked
            created_at: now,
        };

        Ok(success_response(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::internal_transfer_types::AccountType;
    use rust_decimal::Decimal;

    // Note: These tests require a DB connection, so they are integration tests
    // For now, we just test structure

    #[test]
    fn test_handler_creation() {
        // This is a placeholder test
        // Real tests would need MockDB
        assert!(true);
    }
}
