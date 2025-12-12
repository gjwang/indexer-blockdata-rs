// Query endpoint for internal transfer status
use crate::api::{error_response, success_response};
use crate::db::InternalTransferDb;
use crate::models::api_response::ApiResponse;
use crate::models::internal_transfer_types::{AccountType, InternalTransferData, TransferStatus};
use anyhow::Result;
use std::sync::Arc;
use serde_json;

use crate::symbol_manager::SymbolManager;

pub struct InternalTransferQuery {
    pub db: Arc<InternalTransferDb>,
    pub symbol_manager: Arc<SymbolManager>,
}

impl InternalTransferQuery {
    pub fn new(db: Arc<InternalTransferDb>, symbol_manager: Arc<SymbolManager>) -> Self {
        Self { db, symbol_manager }
    }

    /// Get transfer status by request_id
    pub async fn get_transfer_status(
        &self,
        request_id: &str,
    ) -> Result<ApiResponse<Option<InternalTransferData>>> {
        // Parse request_id
        let req_id: i64 = match request_id.parse() {
            Ok(id) => id,
            Err(_) => {
                return Ok(error_response(
                    "INVALID_REQUEST_ID",
                    "Invalid request_id format".to_string(),
                ));
            }
        };

        // Query DB
        let record = match self.db.get_transfer_by_id(req_id).await {
            Ok(Some(rec)) => rec,
            Ok(None) => {
                return Ok(error_response(
                    "NOT_FOUND",
                    format!("Transfer {} not found", request_id),
                ));
            }
            Err(e) => {
                return Ok(error_response(
                    "DB_ERROR",
                    format!("Failed to query transfer: {}", e),
                ));
            }
        };

        // Resolve asset name from ID
        let asset_name = self.symbol_manager.get_asset_name(record.asset_id as u32)
            .unwrap_or_else(|| "UNKNOWN".to_string());

        // Construct AccountTypes - log warning for unknown types
        let from_account = match record.from_account_type.as_str() {
            "funding" => AccountType::Funding { asset: asset_name.clone(), user_id: record.from_user_id as u64 },
            "spot" => AccountType::Spot { user_id: record.from_user_id as u64, asset: asset_name.clone() },
            unknown => {
                log::warn!("Transfer {}: unknown from_account_type '{}'", request_id, unknown);
                AccountType::Funding { asset: asset_name.clone(), user_id: record.from_user_id as u64 }
            }
        };

        let to_account = match record.to_account_type.as_str() {
            "funding" => AccountType::Funding { asset: asset_name.clone(), user_id: record.to_user_id as u64 },
            "spot" => AccountType::Spot { user_id: record.to_user_id as u64, asset: asset_name.clone() },
            unknown => {
                log::warn!("Transfer {}: unknown to_account_type '{}'", request_id, unknown);
                AccountType::Funding { asset: asset_name.clone(), user_id: record.to_user_id as u64 }
            }
        };

        let status = match TransferStatus::from_str(&record.status) {
            Some(s) => s,
            None => {
                log::error!("Transfer {}: corrupt status '{}', defaulting to Failed", request_id, record.status);
                TransferStatus::Failed
            }
        };
        let amount_str = format!("{}.{:08}", record.amount / 100_000_000, record.amount % 100_000_000);

        let data = InternalTransferData {
            request_id: record.request_id.to_string(),
            from_account,
            to_account,
            amount: amount_str,
            status,
            created_at: record.created_at,
        };

        Ok(success_response(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_creation() {
        // Placeholder
        assert!(true);
    }
}
