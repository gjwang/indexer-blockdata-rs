// Transfer history query endpoint
// GET /api/v1/user/internal_transfer/history

use crate::api::{error_response, success_response};
use crate::db::InternalTransferDb;
use crate::models::api_response::ApiResponse;
use crate::models::internal_transfer_types::{AccountType, InternalTransferData, TransferStatus};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Query parameters for history endpoint
#[derive(Debug, Deserialize)]
pub struct TransferHistoryQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
    pub status: Option<String>,
    pub from_date: Option<i64>,
    pub to_date: Option<i64>,
}

/// History response
#[derive(Debug, Serialize)]
pub struct TransferHistoryResponse {
    pub items: Vec<InternalTransferData>,
    pub total: usize,
    pub limit: i32,
    pub offset: i32,
}

pub struct InternalTransferHistory {
    pub db: Arc<InternalTransferDb>,
}

impl InternalTransferHistory {
    pub fn new(db: Arc<InternalTransferDb>) -> Self {
        Self { db }
    }

    /// Get transfer history with pagination
    pub async fn get_history(
        &self,
        user_id: u64,
        query: TransferHistoryQuery,
    ) -> Result<ApiResponse<Option<TransferHistoryResponse>>> {
        let limit = query.limit.unwrap_or(20).min(100); // Max 100
        let offset = query.offset.unwrap_or(0).max(0);

        // Query all transfers for user
        // TODO: Add DB method to query by user_id with pagination
        // For now, return empty result
        let items = vec![];
        let total = 0;

        let response = TransferHistoryResponse {
            items,
            total,
            limit,
            offset,
        };

        Ok(success_response(response))
    }

    /// Get user's recent transfers
    pub async fn get_recent_transfers(
        &self,
        user_id: u64,
        count: i32,
    ) -> Result<Vec<InternalTransferData>> {
        // Query recent transfers
        // TODO: Implement when DB method is ready
        Ok(vec![])
    }

    /// Get transfer statistics for user
    pub async fn get_stats(&self, user_id: u64) -> Result<TransferStats> {
        // TODO: Aggregate stats from DB
        Ok(TransferStats {
            total_transfers: 0,
            successful_transfers: 0,
            failed_transfers: 0,
            pending_transfers: 0,
            total_volume: 0,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct TransferStats {
    pub total_transfers: i64,
    pub successful_transfers: i64,
    pub failed_transfers: i64,
    pub pending_transfers: i64,
    pub total_volume: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_params() {
        let query = TransferHistoryQuery {
            limit: Some(50),
            offset: Some(10),
            status: Some("success".to_string()),
            from_date: None,
            to_date: None,
        };

        assert_eq!(query.limit, Some(50));
        assert_eq!(query.offset, Some(10));
    }

    #[test]
    fn test_stats_structure() {
        let stats = TransferStats {
            total_transfers: 100,
            successful_transfers: 95,
            failed_transfers: 3,
            pending_transfers: 2,
            total_volume: 1000000,
        };

        assert_eq!(stats.total_transfers, 100);
        assert_eq!(stats.successful_transfers, 95);
    }
}
