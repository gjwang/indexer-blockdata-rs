use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;

use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::BalanceRequest;

/// Time window for accepting requests (60 seconds)
/// Requests older than this are REJECTED
const TIME_WINDOW_MS: u64 = 60_000;

/// Time window for tracking request IDs (5 minutes)
/// Must be MUCH LARGER than TIME_WINDOW_MS to prevent replay attacks
/// We keep tracking request IDs for 5 minutes even though we only accept
/// requests within 60 seconds. This ensures that if someone tries to replay
/// an old request, we'll still detect it as a duplicate.
const TRACKING_WINDOW_MS: u64 = TIME_WINDOW_MS * 5; // 5x acceptance window

/// Simulated funding account balances
/// In production, this would be a database tracking external funds
struct SimulatedFundingAccount {
    balances: HashMap<u32, u64>, // asset_id -> amount
}

impl SimulatedFundingAccount {
    fn new() -> Self {
        let mut balances = HashMap::new();
        // Initialize with large amounts for testing
        balances.insert(1, 1_000_000_000_000_000); // BTC
        balances.insert(2, 1_000_000_000_000_000); // USDT
        balances.insert(3, 1_000_000_000_000_000); // ETH
        Self { balances }
    }

    fn has_balance(&self, asset_id: u32, amount: u64) -> bool {
        self.balances
            .get(&asset_id)
            .map_or(false, |&bal| bal >= amount)
    }

    fn reserve(&mut self, asset_id: u32, amount: u64) -> Result<(), String> {
        let balance = self
            .balances
            .get_mut(&asset_id)
            .ok_or_else(|| format!("Asset {} not found in funding account", asset_id))?;

        if *balance < amount {
            return Err(format!(
                "Insufficient funding balance: need {}, have {}",
                amount, balance
            ));
        }

        *balance -= amount;
        Ok(())
    }

    fn release(&mut self, asset_id: u32, amount: u64) {
        *self.balances.entry(asset_id).or_insert(0) += amount;
    }
}

struct BalanceProcessor {
    matching_engine: Arc<Mutex<MatchingEngine>>,
    funding_account: SimulatedFundingAccount,
    /// Track recent request IDs with their timestamps
    recent_requests: HashMap<String, u64>,
    /// Queue of request IDs ordered by timestamp for cleanup
    request_queue: VecDeque<(String, u64)>,
}

impl BalanceProcessor {
    fn new(matching_engine: Arc<Mutex<MatchingEngine>>) -> Self {
        Self {
            matching_engine,
            funding_account: SimulatedFundingAccount::new(),
            recent_requests: HashMap::new(),
            request_queue: VecDeque::new(),
        }
    }

    fn current_time_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn cleanup_old_requests(&mut self) {
        let current_time = self.current_time_ms();

        while let Some((request_id, timestamp)) = self.request_queue.front().cloned() {
            if current_time - timestamp > TRACKING_WINDOW_MS {
                self.recent_requests.remove(&request_id);
                self.request_queue.pop_front();
                println!(
                    "   🧹 Cleaned up old request: {} (age: {}s)",
                    request_id,
                    (current_time - timestamp) / 1000
                );
            } else {
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
                0
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
            println!(
                "   ⚠️  This request_id was already processed. Ignoring to prevent double-spend."
            );
            return Ok(());
        }
        println!("   ✓ Request ID is unique (not seen before)");

        // 3. Process the request (validation passed)
        match req {
            BalanceRequest::TransferIn {
                request_id,
                user_id,
                asset_id,
                amount,
                timestamp,
            } => {
                println!(
                    "\n📥 Processing Transfer In: {} units of asset {} for user {}",
                    amount, asset_id, user_id
                );

                // Phase 1: Check funding account (simulated)
                if !self.funding_account.has_balance(asset_id, amount) {
                    println!("❌ Insufficient funding account balance");
                    return Ok(());
                }

                // Phase 2: Reserve from funding account (simulated)
                if let Err(e) = self.funding_account.reserve(asset_id, amount) {
                    println!("❌ Failed to reserve from funding account: {}", e);
                    return Ok(());
                }
                println!("   ✓ Reserved {} from funding account", amount);

                // Phase 3: Deposit to trading account via MatchingEngine
                let mut me = self.matching_engine.lock().unwrap();
                match me.transfer_in_to_trading_account(user_id, asset_id, amount) {
                    Ok(()) => {
                        // Success! Funds moved from funding -> trading
                        println!("✅ Transfer In completed: {}", request_id);
                        println!(
                            "   Transferred {} from funding_account to user {}'s trading account",
                            amount, user_id
                        );

                        // Track this request
                        self.recent_requests.insert(request_id.clone(), timestamp);
                        self.request_queue.push_back((request_id, timestamp));
                    }
                    Err(e) => {
                        // Failed! Return funds to funding account
                        println!("❌ Failed to transfer in to trading account: {}", e);
                        self.funding_account.release(asset_id, amount);
                        println!("   ↩️  Returned {} to funding account (rollback)", amount);
                    }
                }
            }

            BalanceRequest::TransferOut {
                request_id,
                user_id,
                asset_id,
                amount,
                timestamp,
            } => {
                println!(
                    "\n📤 Processing Transfer Out: {} units of asset {} from user {}",
                    amount, asset_id, user_id
                );

                // Phase 1: Withdraw from trading account via MatchingEngine
                let mut me = self.matching_engine.lock().unwrap();
                match me.transfer_out_from_trading_account(user_id, asset_id, amount) {
                    Ok(()) => {
                        // Success! Funds withdrawn from trading account
                        println!(
                            "   ✓ Withdrawn {} from user {}'s trading account",
                            amount, user_id
                        );

                        // Phase 2: Add to funding account (simulated)
                        self.funding_account.release(asset_id, amount);

                        println!("✅ Transfer Out completed: {}", request_id);
                        println!(
                            "   Transferred {} from user {}'s trading account to funding_account",
                            amount, user_id
                        );

                        // Track this request
                        self.recent_requests.insert(request_id.clone(), timestamp);
                        self.request_queue.push_back((request_id, timestamp));
                    }
                    Err(e) => {
                        println!("❌ Failed to transfer out from trading account: {}", e);
                        println!("   (Likely insufficient balance)");
                    }
                }
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
    let wal_dir = Path::new("me_wal_data");
    let snap_dir = Path::new("me_snapshots");

    // Initialize MatchingEngine (shared with matching_engine_server in production)
    let matching_engine = Arc::new(Mutex::new(
        MatchingEngine::new(wal_dir, snap_dir, config.enable_local_wal).expect("Failed to create MatchingEngine"),
    ));

    let mut processor = BalanceProcessor::new(matching_engine.clone());

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
    println!("Balance Processor Started (Direct ME Integration)");
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Balance Topic:     {}", balance_topic);
    println!("  Consumer Group:    balance_processor_group");
    println!("  WAL Directory:     {:?}", wal_dir);
    println!("  Snapshot Dir:      {:?}", snap_dir);
    println!(
        "  Acceptance Window: {} seconds (requests older than this are rejected)",
        TIME_WINDOW_MS / 1000
    );
    println!(
        "  Tracking Window:   {} seconds (prevents replay attacks)",
        TRACKING_WINDOW_MS / 1000
    );
    println!("--------------------------------------------------");
    println!("Architecture:");
    println!("  Funding Account:   Simulated (Database in production)");
    println!("  Trading Accounts:  MatchingEngine.ledger");
    println!("  Integration:       Direct function call");
    println!("--------------------------------------------------\n");

    let mut total_processed = 0;
    let mut last_report = std::time::Instant::now();

    loop {
        match consumer.recv().await {
            Ok(m) => {
                if let Some(payload) = m.payload_view::<str>() {
                    match payload {
                        Ok(text) => match serde_json::from_str::<BalanceRequest>(text) {
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
                        },
                        Err(e) => eprintln!("Error reading payload: {}", e),
                    }
                }

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
