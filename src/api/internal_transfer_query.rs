// Query endpoint for internal transfer status
use crate::api::{error_response, success_response};
use crate::db::InternalTransferDb;
use crate::models::api_response::ApiResponse;
use crate::models::internal_transfer_types::{AccountType, InternalTransferData, TransferStatus};
use anyhow::Result;
use std::sync::Arc;

pub struct InternalTransferQuery {
    pub db: Arc<InternalTransferDb>,
}

impl InternalTransferQuery {
    pub fn new(db: Arc<InternalTransferDb>) -> Self {
        Self { db }
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

        // Convert to response format
        let from_account = if record.from_account_type == "funding" {
            AccountType::Funding {
                asset: format!("ASSET_{}", record.from_asset_id), // TODO: Get actual asset name
            }
        } else {
            AccountType::Spot {
                user_id: record.from_user_id.unwrap_or(0) as u64,
                asset: format!("ASSET_{}", record.from_asset_id),
            }
        };

        let to_account = if record.to_account_type == "funding" {
            AccountType::Funding {
                asset: format!("ASSET_{}", record.to_asset_id),
            }
        } else {
            AccountType::Spot {
                user_id: record.to_user_id.unwrap_or(0) as u64,
                asset: format!("ASSET_{}", record.to_asset_id),
            }
        };

        let status = TransferStatus::from_str(&record.status).unwrap_or(TransferStatus::Failed);
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
