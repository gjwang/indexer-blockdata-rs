// Settlement handler for internal transfers
// Responsibilities:
// 1. Process UBSCore confirmations (via Kafka)
// 2. POST_PENDING in TigerBeetle
// 3. Scan for stuck transfers (requesting/pending)
// 4. Recovery from crashes

use crate::db::InternalTransferDb;
use tigerbeetle_unofficial::{Client, Transfer};
use tigerbeetle_unofficial::transfer::Flags as TransferFlags;
use crate::models::internal_transfer_types::TransferStatus;
use anyhow::{Result, Context};
use std::sync::Arc;
use tokio::time::{interval, Duration};

// Constants
const SCAN_INTERVAL_MS: u64 = 5_000;  // 5 seconds
const REQUESTING_TIMEOUT_MS: i64 = 24 * 3600 * 1000;  // 24 hours

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
            .set("group.id", &group_id)
            .set("enable.auto.commit", "true")
            .set("auto.offset.reset", "earliest")
            .create()
            .expect("Consumer creation failed");

        consumer.subscribe(&["balance.events"]).expect("Can't subscribe to balance.events");

        log::info!("✅ Internal Transfer Settlement Listener started (Group: {})", group_id);

        loop {
             match consumer.poll(Duration::from_millis(100)) {
                 None => {},
                 Some(Ok(msg)) => {
                     if let Some(payload) = msg.payload() {
                         if let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) {
                             if let Some(event_type) = json.get("event_type").and_then(|v| v.as_str()) {
                                 // Handle Spot -> Funding (Withdraw from Spot) AND Funding -> Spot (Deposit to Spot)
                                 if event_type == "withdraw" || event_type == "deposit" {
                                     let request_id_opt = json.get("ref_id")
                                         .and_then(|v| v.as_u64().map(|id| id as i64))
                                         .or_else(|| {
                                             json.get("ref_id").and_then(|v| v.as_str()).and_then(|s| s.parse::<i64>().ok())
                                         });

                                     if let Some(request_id) = request_id_opt {
                                         log::info!("📥 Received {} Event for Request {}", event_type, request_id);
                                         if let Err(e) = self.process_confirmation(request_id).await {
                                              log::error!("Error processing confirmation {}: {}", request_id, e);
                                         }
                                     } else {
                                         // Some deposits are from External (transfer_in endpoint) and have string IDs e.g. "setup_001"
                                         // These fail parsing to i64 request_id (unless numeric).
                                         // We can ignore them as they are not Internal Transfers tracked by our DB.
                                         // log::debug!("Ignored event with non-numeric ref_id: {:?}", json);
                                     }
                                 }
                             }
                         }
                     }
                 },
                 Some(Err(e)) => log::error!("Kafka error: {}", e),
             }
        }
    }

    /// Process transfer logic (Real TigerBeetle)
    async fn process_confirmation(&self, request_id: i64) -> Result<()> {
        use crate::ubs_core::tigerbeetle::{EXCHANGE_OMNIBUS_ID_PREFIX, tb_account_id};
        use crate::models::internal_transfer_fsm::{InternalTransferStateMachine, InternalTransferEvent, InternalTransferState};

        // 1. Fetch Request from DB
        let record = match self.db.get_transfer_by_id(request_id).await? {
            Some(r) => r,
            None => return Ok(()),
        };

        // Initialize FSM from DB state
        // Simple mapping from string to FSM State
        let current_state = match record.status.as_str() {
            "requesting" => InternalTransferState::Requesting,
            "processing_ubs" => InternalTransferState::Processing,
            "pending" => InternalTransferState::Pending,
            "success" => InternalTransferState::Success,
            "failed" => InternalTransferState::Failed,
            _ => return Err(anyhow::anyhow!("Unknown state in DB: {}", record.status)),
        };

        // If already success, we are done (Idempotent check at state level)
        if current_state == InternalTransferState::Success {
             return Ok(());
        }

        // 2. Validate Transition: Can we Settle from current state?
        // We reconstruct the FSM state.
        let mut fsm = InternalTransferStateMachine { state: current_state };

        // Attempt transition to check validity
        if fsm.consume(InternalTransferEvent::Settle).is_err() {
             // Transition failed.
             log::warn!("Invalid state transition for Settle: {:?} -> Success", current_state);
             // If current state is Failed, don't retry.
             if current_state == InternalTransferState::Failed {
                 return Ok(());
             }
             return Ok(());
        }

        // 3. Execute Transfer in TigerBeetle
        let asset_id = record.asset_id as u32;
        let funding_user_id = (record.to_user_id as u64) | (1u64 << 63);
        let funding_account_id = tb_account_id(funding_user_id, asset_id);
        let omnibus_account_id = tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id);
        let transfer_id = request_id as u128;

        // Check if transfer exists (generic path creates Pending)
        let transfers = self.tb_client.lookup_transfers(vec![transfer_id]).await
            .map_err(|e| anyhow::anyhow!("TB Lookup Failed: {:?}", e))?;

        if let Some(t) = transfers.first() {
             if t.flags().contains(TransferFlags::PENDING) {
                 log::info!("🔄 Found PENDING transfer {}. Posting...", request_id);
                 // POST
                 let post_id = (chrono::Utc::now().timestamp_nanos() as u128) << 16;
                 // Use unique ID for Post operation
                 let post_tx = Transfer::new(post_id)
                     .with_pending_id(t.id())
                     .with_code(t.code())
                     .with_flags(TransferFlags::POST_PENDING_TRANSFER);

                  self.tb_client.create_transfers(vec![post_tx]).await
                      .map_err(|e| anyhow::anyhow!("Failed to POST pending: {:?}", e))?;
                  log::info!("✅ TB Transfer POSTED");
             } else {
                 log::info!("✅ TB Transfer already exists (Finalized).");
             }
        } else {
            // Create NEW (Spot -> Funding path)
            // Ensure accounts exist (Idempotent)
            let _ = crate::ubs_core::tigerbeetle::ensure_account(&self.tb_client, funding_account_id, 1, 100).await;
            let _ = crate::ubs_core::tigerbeetle::ensure_account(&self.tb_client, omnibus_account_id, 1, 100).await;

            let transfer = Transfer::new(transfer_id)
                .with_debit_account_id(omnibus_account_id)
                .with_credit_account_id(funding_account_id)
                .with_amount(record.amount as u128)
                .with_ledger(1)
                .with_code(100);

            match self.tb_client.create_transfers(vec![transfer]).await {
                 Ok(_) => log::info!("✅ TB Transfer created"),
                 Err(e) => {
                     let err_str = format!("{:?}", e);
                     if err_str.contains("Exists") {
                          log::warn!("TB Transfer exists idempotent: {:?}", e);
                     } else if err_str.contains("IdAlreadyFailed") || err_str.contains("Exceeds") || err_str.contains("CreditAccountNotFound") {
                          log::error!("❌ TB Transfer Failed Definitively: {:?}", e);
                          // Transition FSM to Failed
                          let _ = fsm.consume(InternalTransferEvent::Fail);
                          self.db.update_transfer_status(
                              request_id,
                              fsm.as_str(), // "failed"
                              Some(err_str),
                          ).await?;
                          // RETURN Early so we don't mark as success below
                          return Ok(());
                     } else {
                          log::error!("❌ TB Transfer Failed: {:?}", e);
                          return Err(anyhow::anyhow!("TB Transfer Failed: {:?}", e));
                     }
                 }
            }
        }

        // 4. Update DB Status to Success (Using FSM State String)
        // We know fsm.state is now Success because consume(Settle) succeeded.
        self.db.update_transfer_status(
            request_id,
            fsm.as_str(),
            None,
        ).await?;

        log::info!("✅ Transfer {} settled successfully (State: {})", request_id, fsm.as_str());
        Ok(())
    }

    /// Scan for stuck transfers and attempt recovery
    pub async fn scan_stuck_transfers(&self) -> Result<()> {
        // Scan pending transfers
        let pending = self.db.get_transfers_by_status("pending").await?;
        for record in pending {
            if let Err(e) = self.recover_transfer(record.request_id).await {
                log::error!("Failed to recover pending transfer {}: {}", record.request_id, e);
            }
        }

        let requesting = self.db.get_transfers_by_status("requesting").await?;
         for record in requesting {
            if let Err(e) = self.recover_transfer(record.request_id).await {
                log::error!("Failed to recover requesting transfer {}: {}", record.request_id, e);
            }
        }

        Ok(())
    }

    /// Recovery for a specific transfer by checking TB state
    pub async fn recover_transfer(&self, request_id: i64) -> Result<()> {
        let record = match self.db.get_transfer_by_id(request_id).await? {
            Some(rec) => rec,
            None => return Ok(()),
        };

        // Check TB status
        let transfer_id = request_id as u128;
        let transfers = self.tb_client.lookup_transfers(vec![transfer_id]).await
            .context("TB Lookup Failed")?;

        if transfers.is_empty() {
             // Check timeout
             let now = chrono::Utc::now().timestamp_millis();
             let age = now - record.created_at;

             log::info!("🔄 Retrying stuck transaction {} (Empty in TB)", request_id);
             if let Err(e) = self.process_confirmation(request_id).await {
                 log::warn!("Retry failed: {}", e);
                 // If timeout, mark fail
                 if record.status == "requesting" && age >= REQUESTING_TIMEOUT_MS {
                      self.db.update_transfer_status(request_id, TransferStatus::Failed.as_str(), Some("Timeout".to_string())).await?;
                 }
             }
             return Ok(());
        }

        let tb_transfer = &transfers[0];

        // CRITICAL FIX: Check if transfer is PENDING
        if tb_transfer.flags().contains(TransferFlags::PENDING) {
             log::info!("⚠️ Transfer {} found in PENDING state. Posting to finalize...", request_id);

             // We need to POST it.
             let post_id = (chrono::Utc::now().timestamp_nanos() as u128) << 16;

             let post_tx = Transfer::new(post_id)
                 .with_pending_id(tb_transfer.id())
                 .with_code(tb_transfer.code())
                 .with_flags(TransferFlags::POST_PENDING_TRANSFER);

             match self.tb_client.create_transfers(vec![post_tx]).await {
                 Ok(_) => {
                      log::info!("✅ Transfer {} Posted successfully via Recovery", request_id);
                 },
                 Err(e) => {
                      log::error!("Failed to POST transfer {}: {:?}", request_id, e);
                      return Err(anyhow::anyhow!("Failed to post: {:?}", e));
                 }
             }
        }

        // If we are here, we assume it is (or became) successful
        // Update DB if not success
        if record.status != "success" {
             self.db.update_transfer_status(
                    request_id,
                    TransferStatus::Success.as_str(),
                    None,
             ).await?;
             log::info!("Recovered transfer {}: Marked as Success based on TB state", request_id);
        }

        Ok(())
    }

    /// Run continuous scanning (for background task)
    pub async fn run_scanner(self: Arc<Self>) {
        log::info!("🕵️‍♂️ Background Scanner Started");
        let mut ticker = interval(Duration::from_millis(SCAN_INTERVAL_MS));

        loop {
            ticker.tick().await;
            if let Err(e) = self.scan_stuck_transfers().await {
                log::error!("Scan iteration failed: {}", e);
            }
        }
    }
}
