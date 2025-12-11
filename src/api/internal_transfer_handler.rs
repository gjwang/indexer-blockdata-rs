// API Handler for internal transfers
// This is a simplified MVP implementation
//
// Full flow:
// 1. Validate request
// 2. Generate request_id
// 3. Insert DB record (requesting)
// 4. [Future] Create TB PENDING
// 5. [Future] Send to UBSCore via Aeron
// 6. Return response

use crate::api::{error_codes, error_response, success_response, validate_transfer_request, decimal_to_i64};
use crate::db::{TransferRequestRecord, InternalTransferDb};
use crate::models::api_response::ApiResponse;
use crate::models::internal_transfer_types::{InternalTransferData, InternalTransferRequest, TransferStatus};
use crate::symbol_manager::SymbolManager;
use crate::utils::generate_request_id;
use crate::common_utils::get_current_timestamp_ms;
use anyhow::Result;
use std::sync::Arc;

/// Handler state
pub struct InternalTransferHandler {
    pub db: Arc<InternalTransferDb>,
    pub symbol_manager: Arc<SymbolManager>,
}

impl InternalTransferHandler {
    pub fn new(db: Arc<InternalTransferDb>, symbol_manager: Arc<SymbolManager>) -> Self {
        Self { db, symbol_manager }
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

        // 5. Create DB record
        let record = TransferRequestRecord {
            request_id,
            from_account_type: req.from_account.type_name().to_string(),
            from_user_id: req.from_account.user_id().map(|u| u as i64),
            from_asset_id: asset_id as i32,
            to_account_type: req.to_account.type_name().to_string(),
            to_user_id: req.to_account.user_id().map(|u| u as i64),
            to_asset_id: asset_id as i32,
            amount: amount_scaled,
            status: TransferStatus::Requesting.as_str().to_string(),
            created_at: now,
            updated_at: now,
            pending_transfer_id: None,
            posted_transfer_id: None,
            processor: Some("gateway".to_string()),
            error_message: None,
        };

        // 6. Insert to DB
        if let Err(e) = self.db.insert_transfer_request(record).await {
            return Ok(error_response(
                "DB_ERROR",
                format!("Failed to insert transfer: {}", e),
            ));
        }

        // 7. [TODO] Create TB PENDING
        // 8. [TODO] Send to UBSCore

        // 9. Return success
        let data = InternalTransferData {
            request_id: request_id.to_string(),
            from_account: req.from_account,
            to_account: req.to_account,
            amount: req.amount.to_string(),
            status: TransferStatus::Requesting,
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
