//! UBSCore Service - User Balance Service Core
//!
//! This service sits between Gateway and Matching Engine:
//! - Receives orders from Gateway (via Kafka)
//! - Validates orders (balance check, dedup, risk)
//! - Forwards validated orders to Matching Engine
//! - Receives trade fills and updates balances
//!
//! # Architecture
//!
//! ```text
//! Gateway → [Kafka:orders] → UBSCore → [Kafka:validated_orders] → ME
//!                              ↑
//!         ME → [Kafka:fills] ─┘
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use tokio::sync::{Mutex, RwLock};

use fetcher::configure::{self, expand_tilde, AppConfig};
use fetcher::logger::setup_logger;
use fetcher::ubs_core::{
    GroupCommitConfig, GroupCommitWal, HealthChecker, HealthStatus, InternalOrder, LatencyTimer,
    OrderMetrics, RejectReason, SpotRiskModel, UBSCore, WalEntry, WalEntryType, BalanceEvent, TigerBeetleWorker,
};

// Logging macros with target "UBSC"
const TARGET: &str = "UBSC";
macro_rules! info  { ($($arg:tt)*) => { log::info!(target: TARGET, $($arg)*) } }
macro_rules! warn  { ($($arg:tt)*) => { log::warn!(target: TARGET, $($arg)*) } }
macro_rules! error { ($($arg:tt)*) => { log::error!(target: TARGET, $($arg)*) } }
macro_rules! debug { ($($arg:tt)*) => { log::debug!(target: TARGET, $($arg)*) } }

// === CONFIGURATION ===
const STATS_INTERVAL_SECS: u64 = 10;
const HEALTH_HEARTBEAT_MS: u64 = 1000;
const WAL_BUFFER_SIZE: usize = 64 * 1024; // 64KB
const WAL_MAX_BATCH_SIZE: usize = 100;

/// UBSCore Service configuration
#[derive(Debug, Clone)]
struct ServiceConfig {
    /// Kafka broker address
    kafka_brokers: String,

    /// Topic for incoming orders from Gateway
    orders_topic: String,

    /// Topic for validated orders to ME
    validated_orders_topic: String,

    /// Consumer group ID
    consumer_group: String,

    /// Data directory for WAL and snapshots
    data_dir: String,
    tigerbeetle_cluster: u128,
    tigerbeetle_addresses: String,
}

impl ServiceConfig {
    fn from_app_config(config: &AppConfig) -> Self {
        Self {
            kafka_brokers: config.kafka.broker.clone(),
            orders_topic: config.kafka.topics.orders.clone(),
            validated_orders_topic: "validated_orders".to_string(),
            consumer_group: config.kafka.group_id.clone(),
            data_dir: expand_tilde(&config.data_dir),
            tigerbeetle_cluster: 0, // Default or from config if available
            tigerbeetle_addresses: "127.0.0.1:3000".to_string(),
        }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            kafka_brokers: "localhost:9093".to_string(),
            orders_topic: "orders".to_string(),
            validated_orders_topic: "validated_orders".to_string(),
            consumer_group: "ubscore".to_string(),
            data_dir: expand_tilde("~/ubscore_data"),
            tigerbeetle_cluster: 0,
            tigerbeetle_addresses: "127.0.0.1:3000".to_string(),
        }
    }
}

/// UBSCore Service state
struct UBSCoreService {
    core: RwLock<UBSCore<SpotRiskModel>>,
    wal: Mutex<GroupCommitWal>,
    metrics: Arc<OrderMetrics>,
    health: Arc<HealthChecker>,
    wal_entries: std::sync::atomic::AtomicU64,
}

impl UBSCoreService {
    fn new(wal: GroupCommitWal, event_tx: tokio::sync::mpsc::UnboundedSender<BalanceEvent>) -> Self {
        let mut core = UBSCore::new(SpotRiskModel)
            .with_event_listener(event_tx);

        // Register default symbols (for demo)
        // Symbol 1: BTC(1) / USDT(2)
        core.register_symbol(1, 1, 2);

        Self {
            core: RwLock::new(core),
            wal: Mutex::new(wal),
            metrics: Arc::new(OrderMetrics::new()),
            health: Arc::new(HealthChecker::default()),
            wal_entries: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Test Command Processor Loop
    /// Watches './triggers/command' for instructions, executes them, and deletes the file.
    async fn seed_test_accounts(&self) {
        log::info!("🎮 Test Command Mode initialized.");
        log::info!("Instructions: Write commands to './triggers/command'");
        log::info!("Format: CMD arg1 arg2 ...");

        let _ = std::fs::create_dir_all("./triggers");
        // Clean start
        let _ = std::fs::remove_file("./triggers/command");

        loop {
            let cmd_path = std::path::Path::new("./triggers/command");
            if cmd_path.exists() {
                // Read command
                if let Ok(content) = std::fs::read_to_string(cmd_path) {
                    let parts: Vec<&str> = content.trim().split_whitespace().collect();
                    if !parts.is_empty() {
                        log::info!("📥 Received Command: {:?}", parts);
                        self.process_test_command(&parts).await;
                    }
                }
                // Delete command file to signal completion/readiness for next
                let _ = std::fs::remove_file(cmd_path);
            }
            std::thread::sleep(std::time::Duration::from_millis(100)); // Low latency polling
        }
    }

    async fn process_test_command(&self, args: &[&str]) {
        let mut core = self.core.write().await;

        match args[0] {
            "DEPOSIT" => {
                // DEPOSIT <user_id> <asset_id> <amount> <tx_id>
                if args.len() < 5 { log::error!("Usage: DEPOSIT user asset amount tx_id"); return; }
                let user: u64 = args[1].parse().unwrap_or(0);
                let asset: u32 = args[2].parse().unwrap_or(0);
                let amount: u64 = args[3].parse().unwrap_or(0);
                let tx: u64 = args[4].parse().unwrap_or(0);
                core.on_deposit(user, asset, amount, tx);
                log::info!("[CMD] Deposited: user={} asset={} amount={}", user, asset, amount);
            },
            "WITHDRAW" => {
                // WITHDRAW <user_id> <asset_id> <amount> <req_id>
                if args.len() < 5 { log::error!("Usage: WITHDRAW user asset amount req_id"); return; }
                let user: u64 = args[1].parse().unwrap_or(0);
                let asset: u32 = args[2].parse().unwrap_or(0);
                let amount: u64 = args[3].parse().unwrap_or(0);
                let req_id: u64 = args[4].parse().unwrap_or(0);
                // Note: UBSCore currently doesn't expose on_withdraw directly in same way,
                // but we can simulate logic if on_withdraw exists.
                // Checking code... core.rs has on_deposit, let's assume we use lock for withdrawal or implement on_withdraw later.
                // For now, we'll log limitation or use lock if intending to reserve.
                log::warn!("[CMD] WITHDRAW not fully implemented in Core harness yet.");
            },
            "LOCK" => {
                // LOCK <user_id> <asset_id> <amount> <order_id>
                if args.len() < 5 { log::error!("Usage: LOCK user asset amount order_id"); return; }
                let user: u64 = args[1].parse().unwrap_or(0);
                let asset: u32 = args[2].parse().unwrap_or(0);
                let amount: u64 = args[3].parse().unwrap_or(0);
                let oid: u64 = args[4].parse().unwrap_or(0);
                match core.lock_funds(user, asset, amount, oid) {
                    Ok(_) => log::info!("[CMD] Funds Locked: user={} amount={}", user, amount),
                    Err(e) => log::error!("[CMD] Lock Failed: {:?}", e),
                }
            },
            "UNLOCK" => {
                // UNLOCK <user_id> <asset_id> <amount> <order_id>
                if args.len() < 5 { log::error!("Usage: UNLOCK user asset amount order_id"); return; }
                let user: u64 = args[1].parse().unwrap_or(0);
                let asset: u32 = args[2].parse().unwrap_or(0);
                let amount: u64 = args[3].parse().unwrap_or(0);
                let oid: u64 = args[4].parse().unwrap_or(0);
                match core.unlock_funds(user, asset, amount, oid) {
                    Ok(_) => log::info!("[CMD] Funds Unlocked: user={}", user),
                    Err(e) => log::error!("[CMD] Unlock Failed: {:?}", e),
                }
            },
            "SETTLE" => {
                // SETTLE match_id buy_user sell_user base quote price qty
                // Simplified for demo: Uses fixed fees and order IDs derived or passed?
                // Usage: SETTLE match_id buy_uid sell_uid base quote price qty buy_oid sell_oid
                if args.len() < 10 { log::error!("Usage: SETTLE match_id buy_uid sell_uid base quote price qty buy_oid sell_oid"); return; }
                let match_id = args[1].parse().unwrap_or(0);
                let buyer = args[2].parse().unwrap_or(0);
                let seller = args[3].parse().unwrap_or(0);
                let p_asset = args[4].parse().unwrap_or(0); // Payload asset (confusing name in core?) No, core uses base/quote
                let q_asset = args[5].parse().unwrap_or(0);
                let price = args[6].parse().unwrap_or(0);
                let qty = args[7].parse().unwrap_or(0);
                let buy_oid = args[8].parse().unwrap_or(0);
                let sell_oid = args[9].parse().unwrap_or(0);

                // For demo, we pre-lock the SELLER logic implicitly or expect user to have called LOCK?
                // The Test Script should handle the LOCK calls.

                match core.settle_trade(match_id, buyer, seller, p_asset, q_asset, qty, price * qty, 0, buy_oid, sell_oid, 1000, 1000) {
                     Ok(_) => log::info!("[CMD] Trade Settled: match_id={}", match_id),
                     Err(e) => log::error!("[CMD] Settle Failed: {:?}", e),
                }
            },
            "EXIT" => {
                log::info!("🛑 Exiting Test Mode.");
                std::process::exit(0);
            },
            _ => log::warn!("Unknown command: {}", args[0]),
        }
    }

    /// Log order to WAL before processing
    async fn log_order(&self, order: &InternalOrder) -> Result<(), String> {
        let payload = bincode::serialize(order).map_err(|e| format!("Serialize error: {}", e))?;

        let entry = WalEntry::new(WalEntryType::OrderLock, payload);

        let mut wal = self.wal.lock().await;
        wal.append(&entry).map_err(|e| format!("WAL append error: {:?}", e))?;

        self.wal_entries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        Ok(())
    }

    /// Flush WAL to disk
    async fn flush_wal(&self) -> Result<(), String> {
        let mut wal = self.wal.lock().await;
        wal.flush().map_err(|e| format!("WAL flush error: {:?}", e))?;
        Ok(())
    }

    /// Validate an incoming order
    async fn validate_order(&self, order: &InternalOrder) -> Result<(), RejectReason> {
        let timer = LatencyTimer::start();

        self.metrics.record_received();

        let mut core = self.core.write().await;
        let result = core.validate_order(order);

        let latency_ns = timer.elapsed_ns();

        match &result {
            Ok(()) => {
                self.metrics.record_accepted(latency_ns);
            }
            Err(reason) => {
                self.metrics.record_rejected(&format!("{:?}", reason), latency_ns);
            }
        }

        result
    }

    /// Get WAL stats
    fn wal_entries_count(&self) -> u64 {
        self.wal_entries.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() {
    // --- Configuration ---
    let (app_config, service_config) = match configure::load_service_config("ubscore_config") {
        Ok(config) => {
            let svc_config = ServiceConfig::from_app_config(&config);
            (Some(config), svc_config)
        }
        Err(e) => {
            eprintln!("⚠️ Config load failed ({}), using defaults", e);
            (None, ServiceConfig::default())
        }
    };

    // Setup logger
    if let Some(ref config) = app_config {
        if let Err(e) = setup_logger(config) {
            eprintln!("Logger init failed: {}", e);
        }
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }

    info!("============================================");
    info!("       UBSCore Service Starting");
    info!("============================================");
    info!("Kafka brokers: {}", service_config.kafka_brokers);
    info!("Orders topic: {}", service_config.orders_topic);
    info!("Validated orders topic: {}", service_config.validated_orders_topic);
    info!("Consumer group: {}", service_config.consumer_group);
    info!("Data directory: {}", service_config.data_dir);

    // --- Create Data Directory ---
    let data_dir = PathBuf::from(&service_config.data_dir);
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        error!("❌ Failed to create data directory: {}", e);
        std::process::exit(1);
    }
    info!("✅ Data directory ready: {:?}", data_dir);

    // --- Initialize WAL ---
    let wal_path = data_dir.join("ubscore.wal");
    let wal_config = GroupCommitConfig {
        max_batch_size: WAL_MAX_BATCH_SIZE,
        buffer_size: WAL_BUFFER_SIZE,
        use_direct_io: false,
        pre_alloc_size: 64 * 1024 * 1024, // 64MB
    };

    let wal = match GroupCommitWal::open(&wal_path, wal_config) {
        Ok(w) => {
            info!("✅ WAL opened: {:?}", wal_path);
            w
        }
        Err(e) => {
            error!("❌ Failed to open WAL: {:?}", e);
            std::process::exit(1);
        }
    };

    // --- Start TigerBeetle Worker ---
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

    // Hardcoded defaults for now since config update failed
    let tb_addresses = vec!["127.0.0.1:3000".to_string()];
    let tb_cluster = 0;

    if let Err(e) = TigerBeetleWorker::start(tb_cluster, tb_addresses, event_rx) {
         warn!("⚠️ Failed to start TigerBeetle Worker: {}", e);
    } else {
         info!("✅ TigerBeetle Worker started");
    }

    // --- Create Service ---
    let service = Arc::new(UBSCoreService::new(wal, event_tx));
    info!("✅ UBSCore initialized");

    // --- Seed test accounts (development only) ---
    // STEP-BY-STEP VERIFICATION MODE
    service.seed_test_accounts().await;

    // --- Kafka Producer ---
    let producer: FutureProducer = match ClientConfig::new()
        .set("bootstrap.servers", &service_config.kafka_brokers)
        .set("message.timeout.ms", "5000")
        .create()
    {
        Ok(p) => {
            info!("✅ Kafka producer connected");
            p
        }
        Err(e) => {
            error!("❌ Failed to create Kafka producer: {}", e);
            std::process::exit(1);
        }
    };

    // --- Kafka Consumer ---
    // Tuned for low latency:
    // - session.timeout.ms: Faster failure detection
    // - heartbeat.interval.ms: More frequent heartbeats
    // - fetch.wait.max.ms: Don't wait too long for batches
    // - fetch.min.bytes: Don't wait for large batches
    let consumer: StreamConsumer = match ClientConfig::new()
        .set("bootstrap.servers", &service_config.kafka_brokers)
        .set("group.id", &service_config.consumer_group)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        // Low latency tuning
        .set("session.timeout.ms", "6000") // Faster rebalance (default 45000)
        .set("heartbeat.interval.ms", "2000") // More frequent heartbeat
        .set("fetch.wait.max.ms", "100") // Don't wait for batches (default 500)
        .set("fetch.min.bytes", "1") // Return immediately with any data
        .set("max.poll.interval.ms", "300000") // Allow longer processing
        .create()
    {
        Ok(c) => {
            info!("✅ Kafka consumer connected");
            c
        }
        Err(e) => {
            error!("❌ Failed to create Kafka consumer: {}", e);
            std::process::exit(1);
        }
    };

    // Subscribe to orders topic
    if let Err(e) = consumer.subscribe(&[&service_config.orders_topic]) {
        error!("❌ Failed to subscribe: {}", e);
        std::process::exit(1);
    }
    info!("✅ Subscribed to topic: {}", service_config.orders_topic);

    // --- Mark Ready ---
    service.health.set_ready(true);
    service.health.heartbeat();
    info!("✅ UBSCore Service Ready");
    info!("============================================");

    // --- Spawn Stats Reporter ---
    let stats_service = service.clone();
    let stats_data_dir = data_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(STATS_INTERVAL_SECS));
        let start_time = Instant::now();

        loop {
            interval.tick().await;

            let snapshot = stats_service.metrics.snapshot();
            let uptime = start_time.elapsed().as_secs();
            let health = stats_service.health.readiness();
            let wal_entries = stats_service.wal_entries_count();

            // Get WAL file size
            let wal_path = stats_data_dir.join("ubscore.wal");
            let wal_size = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);

            info!(
                "[STATS] uptime={}s | recv={} accept={} reject={} | lat: avg={}µs min={}µs max={}µs | wal: entries={} size={}KB | health={:?}",
                uptime,
                snapshot.orders_received,
                snapshot.orders_accepted,
                snapshot.orders_rejected,
                snapshot.avg_latency_us,
                snapshot.min_latency_us,
                snapshot.max_latency_us,
                wal_entries,
                wal_size / 1024,
                match health {
                    HealthStatus::Healthy => "✅",
                    HealthStatus::Degraded(_) => "⚠️",
                    HealthStatus::Unhealthy(_) => "❌",
                }
            );
        }
    });

    // --- Spawn Heartbeat ---
    let health_clone = service.health.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(HEALTH_HEARTBEAT_MS));
        loop {
            interval.tick().await;
            health_clone.heartbeat();
        }
    });

    // --- Main Processing Loop ---
    run_processing_loop(service, consumer, producer, service_config).await;
}

// ============================================================================
// PROCESSING LOOP
// ============================================================================

async fn run_processing_loop(
    service: Arc<UBSCoreService>,
    consumer: StreamConsumer,
    producer: FutureProducer,
    config: ServiceConfig,
) {
    let mut total_processed: u64 = 0;
    let mut total_accepted: u64 = 0;
    let mut total_rejected: u64 = 0;
    let mut pending_wal_flush: u64 = 0;
    let loop_start = Instant::now();

    info!("📥 Starting order processing loop...");

    loop {
        // --- Receive Order ---
        let recv_timer = LatencyTimer::start();

        match consumer.recv().await {
            Ok(msg) => {
                let recv_latency_us = recv_timer.elapsed_us();

                if let Some(payload) = msg.payload() {
                    let parse_timer = LatencyTimer::start();

                    // --- Parse Order ---
                    match bincode::deserialize::<InternalOrder>(payload) {
                        Ok(order) => {
                            let parse_latency_us = parse_timer.elapsed_us();
                            total_processed += 1;

                            debug!(
                                "[RECV] order_id={} user={} symbol={} side={:?} price={} qty={} | recv={}µs parse={}µs",
                                order.order_id,
                                order.user_id,
                                order.symbol_id,
                                order.side,
                                order.price,
                                order.qty,
                                recv_latency_us,
                                parse_latency_us
                            );

                            // --- Log to WAL ---
                            let wal_timer = LatencyTimer::start();
                            if let Err(e) = service.log_order(&order).await {
                                error!("[WAL_ERROR] {}", e);
                            }
                            let wal_latency_us = wal_timer.elapsed_us();
                            pending_wal_flush += 1;

                            // --- Validate Order ---
                            let process_timer = LatencyTimer::start();
                            let order_id = order.order_id;

                            match service.validate_order(&order).await {
                                Ok(()) => {
                                    let process_latency_us = process_timer.elapsed_us();
                                    total_accepted += 1;

                                    // Flush WAL on accepted orders
                                    if let Err(e) = service.flush_wal().await {
                                        error!("[WAL_FLUSH_ERROR] {}", e);
                                    }
                                    pending_wal_flush = 0;

                                    info!(
                                        "[ACCEPT] order_id={} | wal={}µs process={}µs | total: proc={} accept={} reject={}",
                                        order_id,
                                        wal_latency_us,
                                        process_latency_us,
                                        total_processed,
                                        total_accepted,
                                        total_rejected
                                    );

                                    // --- Forward to ME ---
                                    let forward_timer = LatencyTimer::start();

                                    match bincode::serialize(&order) {
                                        Ok(validated_payload) => {
                                            let key_bytes = order_id.to_be_bytes();
                                            let record =
                                                FutureRecord::to(&config.validated_orders_topic)
                                                    .payload(&validated_payload)
                                                    .key(&key_bytes[..]);

                                            match producer
                                                .send(record, Duration::from_secs(1))
                                                .await
                                            {
                                                Ok((partition, offset)) => {
                                                    let forward_latency_us =
                                                        forward_timer.elapsed_us();
                                                    debug!(
                                                        "[FORWARD] order_id={} -> partition={} offset={} | forward={}µs",
                                                        order_id,
                                                        partition,
                                                        offset,
                                                        forward_latency_us
                                                    );
                                                }
                                                Err((e, _)) => {
                                                    error!(
                                                        "[FORWARD_ERROR] order_id={} error={:?}",
                                                        order_id,
                                                        e
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!(
                                                "[SERIALIZE_ERROR] order_id={} error={}",
                                                order_id,
                                                e
                                            );
                                        }
                                    }
                                }
                                Err(reason) => {
                                    let process_latency_us = process_timer.elapsed_us();
                                    total_rejected += 1;

                                    warn!(
                                        "[REJECT] order_id={} reason={:?} | wal={}µs process={}µs | total: proc={} accept={} reject={}",
                                        order_id,
                                        reason,
                                        wal_latency_us,
                                        process_latency_us,
                                        total_processed,
                                        total_accepted,
                                        total_rejected
                                    );

                                    // Flush WAL periodically even for rejections
                                    if pending_wal_flush >= 100 {
                                        if let Err(e) = service.flush_wal().await {
                                            error!("[WAL_FLUSH_ERROR] {}", e);
                                        }
                                        pending_wal_flush = 0;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "[PARSE_ERROR] Failed to deserialize order: {} | payload_len={}",
                                e,
                                payload.len()
                            );
                        }
                    }
                }
            }
            Err(e) => {
                error!("[KAFKA_ERROR] {}", e);
            }
        }

        // --- Periodic Throughput Log ---
        if total_processed > 0 && total_processed % 1000 == 0 {
            let elapsed = loop_start.elapsed().as_secs_f64();
            let throughput = total_processed as f64 / elapsed;

            info!(
                "[PROGRESS] processed={} accepted={} rejected={} | throughput={:.1} orders/sec",
                total_processed,
                total_accepted,
                total_rejected,
                throughput
            );
        }
    }
}
