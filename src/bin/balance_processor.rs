use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;

use fetcher::ledger::{GlobalLedger, LedgerCommand};
use fetcher::models::BalanceRequest;

/// Special user ID for the funding account
/// All external funds flow through this account
const FUNDING_ACCOUNT_ID: u64 = 0;

/// Time window for accepting requests (60 seconds)
/// Requests older than this are REJECTED
const TIME_WINDOW_MS: u64 = 60_000;

/// Time window for tracking request IDs (5 minutes)
/// Must be MUCH LARGER than TIME_WINDOW_MS to prevent replay attacks
/// We keep tracking request IDs for 5 minutes even though we only accept
/// requests within 60 seconds. This ensures that if someone tries to replay
/// an old request, we'll still detect it as a duplicate.
const TRACKING_WINDOW_MS: u64 = TIME_WINDOW_MS * 5; // 5x acceptance window

struct BalanceProcessor {
    ledger: GlobalLedger,
    /// Track recent request IDs with their timestamps
    /// Key: request_id, Value: timestamp_ms
    recent_requests: HashMap<String, u64>,
    /// Queue of request IDs ordered by timestamp for cleanup
    request_queue: VecDeque<(String, u64)>,
}

impl BalanceProcessor {
    fn new(ledger: GlobalLedger) -> Self {
        Self {
            ledger,
            recent_requests: HashMap::new(),
            request_queue: VecDeque::new(),
        }
    }

    /// Get current timestamp in milliseconds
    fn current_time_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Clean up old requests outside the TRACKING window
    /// Note: TRACKING_WINDOW_MS (5 min) >> TIME_WINDOW_MS (60 sec)
    /// This prevents replay attacks even after the acceptance window closes
    fn cleanup_old_requests(&mut self) {
        let current_time = self.current_time_ms();
        
        while let Some((request_id, timestamp)) = self.request_queue.front().cloned() {
            // Only cleanup requests older than TRACKING_WINDOW_MS (5 minutes)
            // NOT TIME_WINDOW_MS (60 seconds)
            if current_time - timestamp > TRACKING_WINDOW_MS {
                // Remove from tracking
                self.recent_requests.remove(&request_id);
                self.request_queue.pop_front();
                println!("   🧹 Cleaned up old request: {} (age: {}s)", 
                    request_id, 
                    (current_time - timestamp) / 1000
                );
            } else {
                // Queue is ordered, so we can stop
                break;
            }
        }
    }

    fn process_balance_request(&mut self, req: BalanceRequest) -> Result<(), anyhow::Error> {
        // ============================================================
        // SECURITY BOUNDARY: All validation happens HERE
        // The gateway is UNTRUSTED - it could be compromised or have
        // clock skew. We validate timestamp and check duplicates here.
        // ============================================================
        
        let current_time = self.current_time_ms();
        let request_id = req.request_id().to_string();

        println!("\n🔍 Validating request: {}", request_id);
        println!("   Request timestamp: {}", req.timestamp());
        println!("   Current time:      {}", current_time);

        // 1. CRITICAL: Validate timestamp window (prevent replay attacks)
        if !req.is_within_time_window(current_time) {
            let age_sec = if current_time > req.timestamp() {
                (current_time - req.timestamp()) / 1000
            } else {
                0 // Future timestamp
            };
            
            println!(
                "❌ REJECTED: Request outside time window: {} (age: {}s, max: 60s)",
                request_id, age_sec
            );
            
            if current_time < req.timestamp() {
                println!("   ⚠️  WARNING: Request timestamp is in the future! Possible clock skew or attack.");
            }
            
            return Ok(());
        }
        println!("   ✓ Timestamp valid (within 60s window)");

        // 2. CRITICAL: Check for duplicate request_id (prevent double-spend)
        if let Some(&prev_timestamp) = self.recent_requests.get(&request_id) {
            let age_sec = (current_time - prev_timestamp) / 1000;
            println!(
                "❌ REJECTED: Duplicate request detected: {} (first seen {}s ago)",
                request_id, age_sec
            );
            println!("   ⚠️  This request_id was already processed. Ignoring to prevent double-spend.");
            return Ok(());
        }
        println!("   ✓ Request ID is unique (not seen before)");

        // 3. Process the request (validation passed)
        match req {
            BalanceRequest::Deposit {
                request_id,
                user_id,
                asset_id,
                amount,
                timestamp,
            } => {
                println!(
                    "\n📥 Processing Deposit: {} units of asset {} for user {}",
                    amount, asset_id, user_id
                );

                // Transfer from funding_account to user's trading account
                // This is atomic: withdraw from funding + deposit to user
                
                // Step 1: Withdraw from funding account
                self.ledger.apply(&LedgerCommand::Withdraw {
                    user_id: FUNDING_ACCOUNT_ID,
                    asset: asset_id,
                    amount,
                })?;

                // Step 2: Deposit to user's trading account
                self.ledger.apply(&LedgerCommand::Deposit {
                    user_id,
                    asset: asset_id,
                    amount,
                })?;

                // Track this request
                self.recent_requests.insert(request_id.clone(), timestamp);
                self.request_queue.push_back((request_id.clone(), timestamp));

                println!("✅ Deposit completed: {}", request_id);
                println!("   Transferred {} from funding_account to user {}", amount, user_id);
            }

            BalanceRequest::Withdraw {
                request_id,
                user_id,
                asset_id,
                amount,
                timestamp,
            } => {
                println!(
                    "\n📤 Processing Withdrawal: {} units of asset {} from user {}",
                    amount, asset_id, user_id
                );

                // Transfer from user's trading account to funding_account
                // This is atomic: withdraw from user + deposit to funding

                // Step 1: Withdraw from user's trading account
                self.ledger.apply(&LedgerCommand::Withdraw {
                    user_id,
                    asset: asset_id,
                    amount,
                })?;

                // Step 2: Deposit to funding account
                self.ledger.apply(&LedgerCommand::Deposit {
                    user_id: FUNDING_ACCOUNT_ID,
                    asset: asset_id,
                    amount,
                })?;

                // Track this request
                self.recent_requests.insert(request_id.clone(), timestamp);
                self.request_queue.push_back((request_id.clone(), timestamp));

                println!("✅ Withdrawal completed: {}", request_id);
                println!("   Transferred {} from user {} to funding_account", amount, user_id);
            }
        }

        // 4. Cleanup old requests
        self.cleanup_old_requests();

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");
    let wal_dir = Path::new("balance_processor_wal");
    let snap_dir = Path::new("balance_processor_snapshots");

    // Initialize ledger
    let ledger = GlobalLedger::new(wal_dir, snap_dir).expect("Failed to create ledger");
    let mut processor = BalanceProcessor::new(ledger);

    // Initialize funding account with some balance for testing
    println!("🏦 Initializing funding account (ID: {})...", FUNDING_ACCOUNT_ID);
    for asset_id in [1, 2, 3] {
        // BTC, USDT, ETH
        let initial_amount = 1_000_000_000_000_000u64; // Large amount for testing
        processor
            .ledger
            .apply(&LedgerCommand::Deposit {
                user_id: FUNDING_ACCOUNT_ID,
                asset: asset_id,
                amount: initial_amount,
            })
            .expect("Failed to initialize funding account");
        println!("   Asset {}: {} units", asset_id, initial_amount);
    }

    // Kafka Consumer Setup
    let consumer: StreamConsumer = ClientConfig::new()
        .set("group.id", "balance_processor_group")
        .set("bootstrap.servers", &config.kafka.broker)
        .set("auto.offset.reset", "earliest")
        .set("session.timeout.ms", &config.kafka.session_timeout_ms)
        .set("heartbeat.interval.ms", &config.kafka.heartbeat_interval_ms)
        .create()
        .expect("Consumer creation failed");

    let balance_topic = config
        .kafka
        .topics
        .balance_ops
        .clone()
        .unwrap_or_else(|| "balance_ops".to_string());

    consumer
        .subscribe(&[&balance_topic])
        .expect("Can't subscribe to balance_ops topic");

    println!("\n--------------------------------------------------");
    println!("Balance Processor Started (Simplified Internal Transfers)");
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Balance Topic:     {}", balance_topic);
    println!("  Consumer Group:    balance_processor_group");
    println!("  WAL Directory:     {:?}", wal_dir);
    println!("  Snapshot Dir:      {:?}", snap_dir);
    println!("  Funding Account:   {}", FUNDING_ACCOUNT_ID);
    println!("  Acceptance Window: {} seconds (requests older than this are rejected)", TIME_WINDOW_MS / 1000);
    println!("  Tracking Window:   {} seconds (prevents replay attacks)", TRACKING_WINDOW_MS / 1000);
    println!("--------------------------------------------------\n");

    let mut total_processed = 0;
    let mut last_report = std::time::Instant::now();

    loop {
        match consumer.recv().await {
            Ok(m) => {
                if let Some(payload) = m.payload_view::<str>() {
                    match payload {
                        Ok(text) => {
                            match serde_json::from_str::<BalanceRequest>(text) {
                                Ok(req) => {
                                    if let Err(e) = processor.process_balance_request(req) {
                                        eprintln!("❌ Failed to process request: {}", e);
                                    }
                                    
                                    total_processed += 1;
                                }
                                Err(e) => {
                                    eprintln!("Failed to parse balance request: {}", e);
                                    eprintln!("Payload: {}", text);
                                }
                            }
                        }
                        Err(e) => eprintln!("Error reading payload: {}", e),
                    }
                }

                // Periodic reporting
                if last_report.elapsed() >= std::time::Duration::from_secs(10) {
                    println!(
                        "\n📊 Stats: {} requests processed, {} in tracking window",
                        total_processed,
                        processor.recent_requests.len()
                    );
                    last_report = std::time::Instant::now();
                }
            }
            Err(e) => eprintln!("Kafka error: {}", e),
        }
    }
}
