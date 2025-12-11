// Settlement handler for internal transfers
// Responsibilities:
// 1. Process UBSCore confirmations (via Kafka)
// 2. POST_PENDING in TigerBeetle
// 3. Scan for stuck transfers (requesting/pending)
// 4. Recovery from crashes

use crate::db::InternalTransferDb;
use crate::mocks::tigerbeetle_mock::{MockTbClient, TransferStatus as TbStatus};
use crate::models::internal_transfer_types::TransferStatus;
use anyhow::Result;
use std::sync::Arc;
use tokio::time::{interval, Duration};

// Constants
const SCAN_INTERVAL_MS: u64 = 5_000;  // 5 seconds
const REQUESTING_TIMEOUT_MS: i64 = 24 * 3600 * 1000;  // 24 hours
const PENDING_ALERT_MS: i64 = 30 * 60 * 1000;  // 30 minutes
const PENDING_CRITICAL_MS: i64 = 2 * 3600 * 1000;  // 2 hours

pub struct InternalTransferSettlement {
    db: Arc<InternalTransferDb>,
    tb_client: Arc<MockTbClient>,
}

impl InternalTransferSettlement {
    pub fn new(db: Arc<InternalTransferDb>, tb_client: Arc<MockTbClient>) -> Self {
        Self { db, tb_client }
    }

    /// Process transfer confirmation from UBSCore (via Kafka)
    pub async fn process_confirmation(&self, request_id: i64) -> Result<()> {
        log::info!("Processing confirmation for transfer {}", request_id);

        // Get transfer record
        let record = match self.db.get_transfer_by_id(request_id).await? {
            Some(rec) => rec,
            None => {
                log::warn!("Transfer {} not found in DB", request_id);
                return Ok(());
            }
        };

        // Check current status
        if record.status == "success" {
            log::debug!("Transfer {} already marked as success", request_id);
            return Ok(());
        }

        if record.status == "failed" {
            log::warn!("Transfer {} is marked as failed, skipping", request_id);
            return Ok(());
        }

        // POST_PENDING in TigerBeetle
        let transfer_id = request_id as u128;
        match self.tb_client.post_pending_transfer(transfer_id) {
            Ok(_) => {
                // Update DB
                self.db.update_transfer_status(
                    request_id,
                    TransferStatus::Success.as_str(),
                    None,
                ).await?;

                log::info!("✅ Transfer {} completed successfully", request_id);
                Ok(())
            }
            Err(e) => {
                // Retry on next scan
                log::error!("Failed to POST_PENDING for transfer {}: {}", request_id, e);
                Err(e)
            }
        }
    }

    /// Scan for stuck transfers and attempt recovery
    pub async fn scan_stuck_transfers(&self) -> Result<()> {
        log::debug!("Scanning for stuck transfers...");

        // Scan requesting transfers
        let requesting = self.db.get_transfers_by_status("requesting").await?;
        for record in requesting {
            if let Err(e) = self.recover_transfer(record.request_id).await {
                log::error!("Failed to recover requesting transfer {}: {}", record.request_id, e);
            }
        }

        // Scan pending transfers
        let pending = self.db.get_transfers_by_status("pending").await?;
        for record in pending {
            if let Err(e) = self.recover_transfer(record.request_id).await {
                log::error!("Failed to recover pending transfer {}: {}", record.request_id, e);
            }
        }

        Ok(())
    }

    /// Recovery for a specific transfer by checking TB state
    pub async fn recover_transfer(&self, request_id: i64) -> Result<()> {
        log::info!("Attempting recovery for transfer {}", request_id);

        // Get transfer record
        let record = match self.db.get_transfer_by_id(request_id).await? {
            Some(rec) => rec,
            None => {
                log::warn!("Transfer {} not found", request_id);
                return Ok(());
            }
        };

        let now = chrono::Utc::now().timestamp_millis();
        let age = now - record.created_at;

        // Check TB status
        let transfer_id = request_id as u128;
        match self.tb_client.lookup_transfer(transfer_id) {
            Some(tb_status) => {
                match tb_status {
                    TbStatus::Posted => {
                        // TB says it's posted, update DB
                        if record.status != "success" {
                            self.db.update_transfer_status(
                                request_id,
                                TransferStatus::Success.as_str(),
                                None,
                            ).await?;
                            log::info!("Recovered transfer {} to Success (TB: Posted)", request_id);
                        }
                    }
                    TbStatus::Voided => {
                        // TB says it's voided, update DB
                        if record.status != "failed" {
                            self.db.update_transfer_status(
                                request_id,
                                TransferStatus::Failed.as_str(),
                                Some("Voided in TigerBeetle".to_string()),
                            ).await?;
                            log::info!("Recovered transfer {} to Failed (TB: Voided)", request_id);
                        }
                    }
                    TbStatus::Pending => {
                        // Still pending - check timeouts
                        if record.status == "requesting" {
                            // Update to pending since TB has the transfer
                            self.db.update_transfer_status(
                                request_id,
                                TransferStatus::Pending.as_str(),
                                None,
                            ).await?;
                            log::info!("Updated transfer {} from Requesting to Pending", request_id);
                        } else if record.status == "pending" {
                            // Alert based on age
                            if age >= PENDING_CRITICAL_MS {
                                log::error!("🚨🚨 CRITICAL: Transfer {} pending for {}ms", request_id, age);
                            } else if age >= PENDING_ALERT_MS {
                                log::warn!("⚠️  WARNING: Transfer {} pending for {}ms", request_id, age);
                            }
                        }
                    }
                }
            }
            None => {
                // TB doesn't have this transfer
                if record.status == "requesting" && age >= REQUESTING_TIMEOUT_MS {
                    // Too old and TB never got it - mark as failed
                    self.db.update_transfer_status(
                        request_id,
                        TransferStatus::Failed.as_str(),
                        Some("Timeout: TB never received transfer".to_string()),
                    ).await?;
                    log::warn!("Marked transfer {} as Failed (timeout, TB not found)", request_id);
                }
            }
        }

        Ok(())
    }

    /// Run continuous scanning (for background task)
    pub async fn run_scanner(self: Arc<Self>) -> Result<()> {
        let mut ticker = interval(Duration::from_millis(SCAN_INTERVAL_MS));

        loop {
            ticker.tick().await;

            if let Err(e) = self.scan_stuck_transfers().await {
                log::error!("Scan failed: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_settlement_creation() {
        // Placeholder
        assert!(true);
    }
}
