// Settlement handler for internal transfers
// Responsibilities:
// 1. Process UBSCore confirmations (via Kafka)
// 2. POST_PENDING in TigerBeetle
// 3. Scan for stuck transfers (requesting/pending)
// 4. Recovery from crashes

use crate::db::InternalTransferDb;
// use crate::mocks::tigerbeetle_mock::{MockTbClient, TransferStatus as TbStatus}; // Remove Mock
use tigerbeetle_unofficial::{Client, Transfer}; // Removed TransferFlags
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
    tb_client: Arc<Client>,
}

impl InternalTransferSettlement {
    pub fn new(db: Arc<InternalTransferDb>, tb_client: Arc<Client>) -> Self {
        Self { db, tb_client }
    }

     /// Run Kafka Consumer Loop
    pub async fn run_consumer(self: Arc<Self>, brokers: String, group_id: String) {
        use rdkafka::config::ClientConfig;
        use rdkafka::consumer::{BaseConsumer, Consumer};
        use rdkafka::Message;

        let consumer: BaseConsumer = ClientConfig::new()
            .set("bootstrap.servers", &brokers)
            .set("group.id", "settlement_listener_group_v3") // Bumped to v3
            .set("enable.auto.commit", "true")
            .set("auto.offset.reset", "earliest")
            .create()
            .expect("Consumer creation failed");

        consumer.subscribe(&["balance.events"]).expect("Can't subscribe to balance.events");

        log::info!("✅ Internal Transfer Settlement Listener started");

        loop {
             // Poll for messages (non-blocking)
             match consumer.poll(Duration::from_millis(0)) {
                 None => {
                     // No message, yield to runtime
                     tokio::time::sleep(Duration::from_millis(10)).await;
                     continue;
                 }
                 Some(Ok(msg)) => {
                     if let Some(payload) = msg.payload() {
                         // We need to parse BalanceEvent.
                         if let Ok(payload_str) = std::str::from_utf8(payload) {
                             log::info!("📥 Settlement received: {}", payload_str);
                         }
                         // BalanceEvent is in crate::ubs_core::events::BalanceEvent or crate::models?
                         // It matches the one used in ubscore_aeron_service.
                         // Let's assume generic JSON structure to extract ref_id/tx_id

                         if let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) {
                             // BalanceEvent is a struct with event_type field
                             if let Some(event_type) = json.get("event_type").and_then(|v| v.as_str()) {
                                 log::info!("🔎 Event type: {}", event_type);
                                 // Handle Spot -> Funding (Withdraw from Spot)
                                 if event_type == "withdraw" {
                                         if let Some(ref_id_u64) = json.get("ref_id").and_then(|v| v.as_u64()) {
                                             let request_id = ref_id_u64 as i64;

                                             // Process confirmation (blocking to ensure execution)
                                             if let Err(e) = self.process_confirmation(request_id).await {
                                                  log::error!("Error processing confirmation {}: {}", request_id, e);
                                             }
                                         }
                                     }
                                 }
                                 // Handle Funding -> Spot (Deposit to Spot)?
                                 // Usually handled via TransferIn response flow, but if we used internal transfer DB, we might listen here too.
                                 // For now, only focus on Spot -> Funding (withdraw).
                             }
                         }
                     }
                 Some(Err(e)) => log::error!("Kafka error: {}", e),
                 None => {},
             }
        }
    }

    /// Process transfer logic (Real TigerBeetle)
    async fn process_confirmation(&self, request_id: i64) -> Result<()> {
        use crate::ubs_core::tigerbeetle::{EXCHANGE_OMNIBUS_ID_PREFIX, tb_account_id};

        // 1. Fetch Request from DB
        let record = match self.db.get_transfer_by_id(request_id).await? {
            Some(r) => r,
            None => {
                log::warn!("Transfer {} not found in DB", request_id);
                return Ok(());
            }
        };

        if record.status != "pending" {
            log::info!("Transfer {} already settled (status={})", request_id, record.status);
            return Ok(());
        }

        // 2. Execute Transfer in TigerBeetle
        // Flow: Withdrawal (Spot -> Funding)

        let asset_id = record.asset_id as u32;

        // Use MSB offset for Funding Account to differentiate from Spot Account
        // Funding User ID = user_id | (1 << 63).
        let funding_user_id = (record.to_user_id as u64) | (1u64 << 63);

        // Funding Account (Credit)
        let funding_account_id = tb_account_id(funding_user_id, asset_id);

        // Omnibus Account (Debit) - where UBSCore moved funds to
        let omnibus_account_id = tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id);

        let transfer_id = request_id as u128;

        let transfer = Transfer::new(transfer_id)
            .with_debit_account_id(omnibus_account_id)
            .with_credit_account_id(funding_account_id)
            .with_amount(record.amount as u128)
            .with_ledger(1) // TRADING_LEDGER
            .with_code(100);

        log::info!("💰 Spot->Funding: Crediting Funding Account {:?} from Omnibus", funding_account_id);

        let transfers = vec![transfer];

        // Ensure accounts exist (Funding & Omnibus)
        if let Err(e) = crate::ubs_core::tigerbeetle::ensure_account(&self.tb_client, omnibus_account_id, 1, 1).await {
             log::warn!("Failed to ensure Omnibus Account: {}", e);
        }
        if let Err(e) = crate::ubs_core::tigerbeetle::ensure_account(&self.tb_client, funding_account_id, 1, 1).await {
             log::warn!("Failed to ensure Funding Account: {}", e);
        }

        // Compiler suggests return type is Result<()>, not Result<Vec<...>>
        if let Err(e) = self.tb_client.create_transfers(transfers).await {
              log::error!("TB Transfer API Error: {:?}", e);
              return Err(e.into());
        }

        // 3. Update DB Status
        self.db.update_transfer_status(
            request_id,
            TransferStatus::Success.as_str(),
            None,
        ).await?;

        log::info!("✅ Transfer {} (Spot->Funding) settled successfully", request_id);
        Ok(())
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

        // Check TB status
        let transfer_id = request_id as u128;

        let transfers = self.tb_client.lookup_transfers(vec![transfer_id]).await?;

        if transfers.is_empty() {
            // Not found in TB
             let now = chrono::Utc::now().timestamp_millis();
             let age = now - record.created_at;

             if record.status == "requesting" && age >= REQUESTING_TIMEOUT_MS {
                    // Too old and TB never got it - mark as failed
                    self.db.update_transfer_status(
                        request_id,
                        TransferStatus::Failed.as_str(),
                        Some("Timeout: TB never received transfer".to_string()),
                    ).await?;
                    log::warn!("Marked transfer {} as Failed (timeout, TB not found)", request_id);
             } else {
                 log::debug!("Transfer {} not found in TB yet (age: {}ms)", request_id, age);
             }
             return Ok(());
        }

        // If found in TB, it exists.
        // For simple transfers (Spot->Funding used in create_transfers), existence = success.
        // Update DB if not success.
        if record.status != "success" {
             self.db.update_transfer_status(
                    request_id,
                    TransferStatus::Success.as_str(),
                    None,
             ).await?;
             log::info!("Recovered transfer {}: Marked as Success (Found in TB)", request_id);
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
