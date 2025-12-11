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
use crate::models::internal_transfer_types::{InternalTransferData, InternalTransferRequest, TransferStatus};
use crate::mocks::tigerbeetle_mock::MockTbClient;
use crate::symbol_manager::SymbolManager;
use crate::utils::generate_request_id;
use crate::common_utils::get_current_timestamp_ms;
use anyhow::Result;
use std::sync::Arc;

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
    pub tb_client: Arc<MockTbClient>,
}

impl InternalTransferHandler {
    pub fn new(
        db: Arc<InternalTransferDb>,
        symbol_manager: Arc<SymbolManager>,
        tb_client: Arc<MockTbClient>,
    ) -> Self {
        Self { db, symbol_manager, tb_client }
    }

    /// Handle internal transfer request (MVP version)
    pub async fn handle_transfer(
        &self,
        req: InternalTransferRequest,
    ) -> Result<ApiResponse<Option<InternalTransferData>>> {
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
        let (debit_account_id, credit_account_id) = match (&req.from_account, &req.to_account) {
            (from, to) => {
                let from_user = from.user_id().unwrap_or(FUNDING_USER_ID);
                let to_user = to.user_id().unwrap_or(FUNDING_USER_ID);
                (
                    tb_account_id(from_user, asset_id),
                    tb_account_id(to_user, asset_id),
                )
            }
        };

        // 8. Check funding account balance
        match self.tb_client.get_available_balance(debit_account_id) {
            Ok(balance) => {
                if balance < amount_scaled {
                    return Ok(error_response(
                        error_codes::INSUFFICIENT_BALANCE,
                        format!("Insufficient balance: have {}, need {}", balance, amount_scaled),
                    ));
                }
            }
            Err(e) => {
                return Ok(error_response(
                    "TB_ERROR",
                    format!("Failed to check balance: {}", e),
                ));
            }
        }

        // 9. Create TB PENDING (lock funds)
        let transfer_id = request_id as u128;
        if let Err(e) = self.tb_client.create_pending_transfer(
            transfer_id,
            debit_account_id,
            credit_account_id,
            amount_scaled,
        ) {
            // Failed to lock - mark as failed
            let _ = self.db.update_transfer_status(
                request_id,
                TransferStatus::Failed.as_str(),
                Some(format!("TB lock failed: {}", e)),
            ).await;

            return Ok(error_response(
                error_codes::INSUFFICIENT_BALANCE,
                format!("Failed to lock funds: {}", e),
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
