// Admin tools for internal transfer management
// Provides admin-only operations for manual intervention

use crate::db::InternalTransferDb;
use crate::mocks::tigerbeetle_mock::MockTbClient;
use crate::models::internal_transfer_types::TransferStatus;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Admin operations for internal transfers
pub struct InternalTransferAdmin {
    db: Arc<InternalTransferDb>,
    tb_client: Arc<MockTbClient>,
}

impl InternalTransferAdmin {
    pub fn new(db: Arc<InternalTransferDb>, tb_client: Arc<MockTbClient>) -> Self {
        Self { db, tb_client }
    }

    /// Manually VOID a pending transfer (admin only)
    /// Use case: User reported issue, manual verification needed
    pub async fn manual_void(&self, request_id: i64, reason: String) -> Result<AdminActionResult> {
        log::warn!("ADMIN ACTION: Manual VOID for transfer {}: {}", request_id, reason);

        // Get transfer
        let record = self.db.get_transfer_by_id(request_id).await?
            .ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;

        // Check status
        if record.status == "success" {
            return Ok(AdminActionResult {
                success: false,
                message: "Cannot VOID completed transfer".to_string(),
                details: None,
            });
        }

        if record.status == "failed" {
            return Ok(AdminActionResult {
                success: false,
                message: "Transfer already failed".to_string(),
                details: None,
            });
        }

        // VOID in TB
        let transfer_id = request_id as u128;
        match self.tb_client.void_pending_transfer(transfer_id) {
            Ok(_) => {
                // Update DB
                self.db.update_transfer_status(
                    request_id,
                    TransferStatus::Failed.as_str(),
                    Some(format!("Manual VOID: {}", reason)),
                ).await?;

                Ok(AdminActionResult {
                    success: true,
                    message: "Transfer voided successfully".to_string(),
                    details: Some(format!("Funds returned to source account")),
                })
            }
            Err(e) => {
                Ok(AdminActionResult {
                    success: false,
                    message: format!("Failed to VOID: {}", e),
                    details: None,
                })
            }
        }
    }

    /// Manually force POST a pending transfer (admin only)
    /// Use case: Settlement failed but TB shows pending, manual completion
    pub async fn manual_post(&self, request_id: i64, reason: String) -> Result<AdminActionResult> {
        log::warn!("ADMIN ACTION: Manual POST for transfer {}: {}", request_id, reason);

        // Get transfer
        let record = self.db.get_transfer_by_id(request_id).await?
            .ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;

        if record.status == "success" {
            return Ok(AdminActionResult {
                success: false,
                message: "Transfer already completed".to_string(),
                details: None,
            });
        }

        // POST in TB
        let transfer_id = request_id as u128;
        match self.tb_client.post_pending_transfer(transfer_id) {
            Ok(_) => {
                // Update DB
                self.db.update_transfer_status(
                    request_id,
                    TransferStatus::Success.as_str(),
                    None,
                ).await?;

                Ok(AdminActionResult {
                    success: true,
                    message: "Transfer completed successfully".to_string(),
                    details: Some(format!("Manual POST: {}", reason)),
                })
            }
            Err(e) => {
                Ok(AdminActionResult {
                    success: false,
                    message: format!("Failed to POST: {}", e),
                    details: Some(e.to_string()),
                })
            }
        }
    }

    /// Get stuck transfers report (pending > threshold)
    pub async fn get_stuck_transfers(&self, threshold_ms: i64) -> Result<Vec<StuckTransferReport>> {
        let now = chrono::Utc::now().timestamp_millis();

        // Query pending transfers
        let pending = self.db.get_transfers_by_status("pending").await?;

        let mut stuck = vec![];
        for record in pending {
            let age = now - record.created_at;
            if age >= threshold_ms {
                // Check TB status
                let tb_status = self.tb_client.lookup_transfer(record.request_id as u128);

                stuck.push(StuckTransferReport {
                    request_id: record.request_id,
                    age_ms: age,
                    amount: record.amount,
                    from_user_id: record.from_user_id,
                    to_user_id: record.to_user_id,
                    tb_status: format!("{:?}", tb_status),
                    created_at: record.created_at,
                });
            }
        }

        Ok(stuck)
    }

    /// Reconcile transfer state with TigerBeetle
    pub async fn reconcile_transfer(&self, request_id: i64) -> Result<ReconciliationReport> {
        let db_record = self.db.get_transfer_by_id(request_id).await?
            .ok_or_else(|| anyhow::anyhow!("Transfer not found in DB"))?;

        let tb_status = self.tb_client.lookup_transfer(request_id as u128);

        let matches = match tb_status {
            Some(crate::mocks::tigerbeetle_mock::TransferStatus::Pending) => {
                db_record.status == "pending" || db_record.status == "requesting"
            }
            Some(crate::mocks::tigerbeetle_mock::TransferStatus::Posted) => {
                db_record.status == "success"
            }
            Some(crate::mocks::tigerbeetle_mock::TransferStatus::Voided) => {
                db_record.status == "failed"
            }
            None => db_record.status == "requesting" || db_record.status == "failed",
        };

        Ok(ReconciliationReport {
            request_id,
            db_status: db_record.status.clone(),
            tb_status: format!("{:?}", tb_status),
            matches,
            recommendation: if !matches {
                Some("Manual intervention required - state mismatch".to_string())
            } else {
                None
            },
        })
    }
}

#[derive(Debug, Serialize)]
pub struct AdminActionResult {
    pub success: bool,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StuckTransferReport {
    pub request_id: i64,
    pub age_ms: i64,
    pub amount: i64,
    pub from_user_id: Option<i64>,
    pub to_user_id: Option<i64>,
    pub tb_status: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ReconciliationReport {
    pub request_id: i64,
    pub db_status: String,
    pub tb_status: String,
    pub matches: bool,
    pub recommendation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_action_result() {
        let result = AdminActionResult {
            success: true,
            message: "Operation successful".to_string(),
            details: Some("Transfer voided".to_string()),
        };

        assert!(result.success);
        assert_eq!(result.message, "Operation successful");
    }

    #[test]
    fn test_stuck_transfer_report() {
        let report = StuckTransferReport {
            request_id: 123,
            age_ms: 3600000, // 1 hour
            amount: 1000000,
            from_user_id: Some(1),
            to_user_id: Some(2),
            tb_status: "Pending".to_string(),
            created_at: 1234567890,
        };

        assert_eq!(report.age_ms, 3600000);
        assert!(report.age_ms > 1800000); // > 30 minutes
    }
}
