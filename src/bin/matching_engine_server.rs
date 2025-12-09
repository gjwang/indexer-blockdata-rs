use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};

use disruptor::*;

use fetcher::engine_output::EngineOutput;
use fetcher::ledger::LedgerCommand;
use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::client_order::calculate_checksum;
use fetcher::models::{BalanceRequest, OrderRequest, OrderType, Side};
use fetcher::symbol_manager::SymbolManager;
use fetcher::zmq_publisher::ZmqPublisher;

/// Event structure for the Disruptor ring buffer
struct OrderEvent {
    command: Option<EngineCommand>,
    /// Atomic output bundles for all operations (EngineOutput flow)
    engine_outputs: std::sync::Mutex<Option<std::sync::Arc<Vec<EngineOutput>>>>,
    kafka_offset: Option<(String, i32, i64)>,
    timestamp: u64,
}

#[derive(Clone)]
enum EngineCommand {
    PlaceOrderBatch(Vec<(u32, u64, Side, OrderType, u64, u64, u64, u64)>),
    CancelOrder { symbol_id: u32, order_id: u64 },
    BalanceRequest(BalanceRequest),
}

/// Time window for accepting requests (60 seconds)
const TIME_WINDOW_MS: u64 = 60_000;
/// Time window for tracking request IDs (5 minutes)
const TRACKING_WINDOW_MS: u64 = TIME_WINDOW_MS * 5;
/// Max Kafka offsets to track for deduplication
const MAX_TRACKED_OFFSETS: usize = 100_000;

/// Tracks recently processed Kafka message offsets to detect duplicates
/// Critical for at-least-once delivery: Kafka may redeliver on crash recovery
///
/// Safety: Only accepts messages within the tracked time window.
/// If a message is older than the oldest tracked offset, we cannot verify
/// if it was already processed → REJECT as potentially dangerous.
struct MessageDeduplicator {
    /// Set of recently seen Kafka offsets (topic+partition+offset hash)
    recent_offsets: std::collections::HashSet<i64>,
    /// Queue of (offset, timestamp) for FIFO eviction and oldest-ts tracking
    offset_queue: VecDeque<(i64, u64)>,
    /// Timestamp of the oldest tracked message (defines the safe window)
    oldest_tracked_ts: u64,
}

impl MessageDeduplicator {
    fn new() -> Self {
        Self {
            recent_offsets: std::collections::HashSet::with_capacity(MAX_TRACKED_OFFSETS),
            offset_queue: VecDeque::with_capacity(MAX_TRACKED_OFFSETS),
            oldest_tracked_ts: 0, // 0 means no messages tracked yet
        }
    }

    /// Check if Kafka offset was already processed. Returns true if duplicate.
    fn is_duplicate(&self, offset: i64) -> bool {
        self.recent_offsets.contains(&offset)
    }

    /// Check if message timestamp is outside our tracked window
    /// Only applies when queue is FULL - before that, rely on ID check alone
    /// Returns true if message should be rejected (too old to verify dedup)
    fn is_outside_tracked_window(&self, msg_ts: u64) -> bool {
        // Only enforce timestamp window when queue is at capacity
        // Before full, we can still dedup by ID since queue has room
        if self.offset_queue.len() < MAX_TRACKED_OFFSETS {
            return false; // Queue not full, rely on ID check
        }
        if msg_ts == 0 || self.oldest_tracked_ts == 0 {
            return false; // No timestamp info available
        }
        // Queue is full - reject if older than oldest tracked message
        msg_ts < self.oldest_tracked_ts
    }

    /// Get the oldest tracked timestamp (for logging)
    fn get_oldest_ts(&self) -> u64 {
        self.oldest_tracked_ts
    }

    /// Mark Kafka offset as processed with its timestamp
    fn mark_processed(&mut self, offset: i64, timestamp: u64) {
        if self.recent_offsets.insert(offset) {
            self.offset_queue.push_back((offset, timestamp));

            // Evict oldest if over limit
            while self.offset_queue.len() > MAX_TRACKED_OFFSETS {
                if let Some((oldest_offset, _oldest_ts)) = self.offset_queue.pop_front() {
                    self.recent_offsets.remove(&oldest_offset);
                }
            }

            // Update oldest_tracked_ts from front of queue
            self.oldest_tracked_ts =
                self.offset_queue.front().map(|(_offset, ts)| *ts).unwrap_or(0);
        }
    }
}

// REMOVED: BalanceProcessor - Deposits and withdrawals are now handled by UBSCore
// ME no longer processes balance operations
/*
struct BalanceProcessor {
    recent_requests: HashMap<String, u64>,
    request_queue: VecDeque<(String, u64)>,
}

impl BalanceProcessor {
    ... (removed 80+ lines)
}
*/

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    // Initialize ZMQ Publisher
    let zmq_config = config.zeromq.as_ref().expect("ZMQ config missing");
    let zmq_publisher = std::sync::Arc::new(
        ZmqPublisher::new(zmq_config.settlement_port, zmq_config.market_data_port)
            .expect("Failed to bind ZMQ sockets"),
    );

    // Snapshot directory for engine state persistence
    let snap_dir_str = std::env::var("APP_SNAP_DIR").unwrap_or_else(|_| "me_snapshots".to_string());
    let snap_dir = Path::new(&snap_dir_str);

    // Clean up previous run (development mode - fresh start each time)
    if snap_dir.exists() {
        let _ = fs::remove_dir_all(snap_dir);
    }
    // Also clean ledger WAL to prevent recovery from stale state
    let ledger_wal_dir = Path::new("ledger_wal");
    if ledger_wal_dir.exists() {
        let _ = fs::remove_dir_all(ledger_wal_dir);
    }

    // Create engine without WAL (EngineOutput provides deterministic replay)
    let dummy_wal_dir = Path::new("/tmp/me_dummy_wal");
    if dummy_wal_dir.exists() {
        let _ = fs::remove_dir_all(dummy_wal_dir);
    }
    let mut engine =
        MatchingEngine::new(dummy_wal_dir, snap_dir, false).expect("Failed to create engine");

    // === Initialize Symbols & Funds (Hardcoded for Demo) ===
    println!("=== Initializing Engine State ===");
    let symbol_manager = SymbolManager::load_from_db();

    // Register Symbols
    for (&symbol_id, symbol) in &symbol_manager.id_to_symbol {
        let (base, quote) = match symbol.as_str() {
            "BTC_USDT" => (1, 2),
            "ETH_USDT" => (3, 2),
            _ => (100, 2),
        };
        engine.register_symbol(symbol_id, symbol.clone(), base, quote).unwrap();
        println!("Loaded symbol: {}", symbol);
    }

    // === Kafka Consumer Setup ===
    let consumer_raw: StreamConsumer = ClientConfig::new()
        .set("group.id", &config.kafka.group_id)
        .set("bootstrap.servers", &config.kafka.broker)
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.offset.store", "false") // CRITICAL: Only store offset after WAL flush
        .set("session.timeout.ms", &config.kafka.session_timeout_ms)
        .set("heartbeat.interval.ms", &config.kafka.heartbeat_interval_ms)
        .set("fetch.wait.max.ms", &config.kafka.fetch_wait_max_ms)
        .set("max.poll.interval.ms", &config.kafka.max_poll_interval_ms)
        .set("socket.keepalive.enable", &config.kafka.socket_keepalive_enable)
        .create()
        .expect("Consumer creation failed");
    let consumer = std::sync::Arc::new(consumer_raw);

    // === Kafka Producer Setup ===
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Producer creation failed");

    // REMOVED: Balance operations now handled by UBSCore, not ME
    // let balance_topic = config.kafka.topics.balance_ops.clone().unwrap_or("balance.operations".to_string());
    // let mut balance_processor = BalanceProcessor::new();

    consumer
        .subscribe(&[&config.kafka.topics.orders]) // Only subscribe to orders topic
        .expect("Subscription failed");

    println!("--------------------------------------------------");
    println!("Boot Parameters:");
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Orders Topic:      {}", config.kafka.topics.orders);
    println!("  Trades Topic:      {}", config.kafka.topics.trades);
    println!("  Consumer Group:    {}", config.kafka.group_id);
    println!("  Snapshot Dir:      {:?}", snap_dir);
    println!("--------------------------------------------------");
    println!(">>> Matching Engine Server Started (EngineOutput Mode)");

    // === Wait for Settlement Service to be ready ===
    println!(">>> Waiting for Settlement Service READY signal...");
    let zmq_ctx = zmq::Context::new();
    let sync_req = zmq_ctx.socket(zmq::REQ).expect("Failed to create REQ socket");
    let sync_port = config.zeromq.as_ref().unwrap().settlement_port + 2;
    sync_req
        .connect(&format!("tcp://localhost:{}", sync_port))
        .expect("Failed to connect to sync port");
    sync_req.send(&b"PING"[..], 0).expect("Failed to send PING");

    match sync_req.recv_bytes(0) {
        Ok(msg) if msg == b"READY" => {
            println!(">>> ✅ Settlement Service is READY");
        }
        Ok(_) => {
            eprintln!("Unexpected response from Settlement Service");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to receive READY from Settlement: {}", e);
            std::process::exit(1);
        }
    }
    drop(sync_req); // Close sync socket

    let mut total_orders = 0;
    let mut last_report = std::time::Instant::now();

    let batch_poll_count = 1000;

    // === Disruptor Pipeline Architecture ===
    // Message deduplicator for at-least-once delivery safety
    // Uses business-level unique IDs from message content
    let mut msg_dedup = MessageDeduplicator::new();

    // Factory: Create empty events in the ring buffer
    let factory = || OrderEvent {
        command: None,
        engine_outputs: std::sync::Mutex::new(None),
        kafka_offset: None,
        timestamp: 0,
    };

    // === Consumer 1: Matcher (processes all commands and produces EngineOutput) ===
    let matcher = move |event: &OrderEvent, _sequence: Sequence, _end_of_batch: bool| {
        let msg_ts = event.timestamp;

        // Check for messages outside our tracked dedup window
        if msg_dedup.is_outside_tracked_window(msg_ts) {
            println!(
                "[ME] OUTSIDE TRACKED WINDOW - ts={} < oldest={}",
                msg_ts,
                msg_dedup.get_oldest_ts()
            );
            return;
        }

        if let Some(ref cmd) = event.command {
            match cmd {
                // Batch: check each order_id individually
                EngineCommand::PlaceOrderBatch(batch) => {
                    let mut engine_outputs = Vec::with_capacity(batch.len());
                    let mut input_seq = engine.get_output_seq();

                    for (
                        symbol_id,
                        order_id,
                        side,
                        order_type,
                        price,
                        quantity,
                        user_id,
                        timestamp,
                    ) in batch.iter()
                    {
                        let msg_id = *order_id as i64;
                        if msg_dedup.is_duplicate(msg_id) {
                            continue;
                        }
                        msg_dedup.mark_processed(msg_id, msg_ts);

                        input_seq += 1;
                        if let Ok((_, output)) = engine.add_order_and_build_output(
                            input_seq,
                            *symbol_id,
                            *order_id,
                            *side,
                            *order_type,
                            *price,
                            *quantity,
                            *user_id,
                            *timestamp,
                            format!("kafka_{}", order_id),
                        ) {
                            engine_outputs.push(output);
                        }
                    }

                    if !engine_outputs.is_empty() {
                        *event.engine_outputs.lock().unwrap() =
                            Some(std::sync::Arc::new(engine_outputs));
                    }
                }
                // Single ID: check once at top
                EngineCommand::CancelOrder { symbol_id, order_id } => {
                    let msg_id = *order_id as i64;
                    if msg_dedup.is_duplicate(msg_id) {
                        return;
                    }
                    msg_dedup.mark_processed(msg_id, msg_ts);
                    let _ = engine.cancel_order(*symbol_id, *order_id);
                }
                // Balance operations now handled by UBSCore - ignore these messages
                EngineCommand::BalanceRequest(_) => {
                    // NO-OP: ME no longer processes balance operations
                }
            }
        }
    };

    // === Consumer 2: ZMQ Publisher + Offset Committer (AFTER Matcher) ===
    // CRITICAL: Offset must be committed AFTER processing to prevent message loss
    let zmq_pub_clone = zmq_publisher.clone();
    let consumer_for_commit = consumer.clone();

    // Profiling counters for ZMQ output
    let zmq_output_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let zmq_trade_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let zmq_start_time = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let zmq_last_report = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

    let zmq_out_cnt = zmq_output_count.clone();
    let zmq_trd_cnt = zmq_trade_count.clone();
    let zmq_start = zmq_start_time.clone();
    let zmq_last = zmq_last_report.clone();

    let zmq_and_commit = move |event: &OrderEvent, _sequence: Sequence, end_of_batch: bool| {
        // 1. Publish EngineOutput bundles
        let outputs_arc = { event.engine_outputs.lock().unwrap().as_ref().cloned() };
        if let Some(outputs) = outputs_arc {
            for output in outputs.iter() {
                // Publish market data (trades) from EngineOutput
                for trade in &output.trades {
                    if let Ok(data) = serde_json::to_vec(trade) {
                        let _ = zmq_pub_clone.publish_market_data(&data);
                    }
                }

                // Publish EngineOutput bundle to Settlement
                if let Err(e) = zmq_pub_clone.publish_engine_output(output) {
                    eprintln!("[ZMQ ERROR] Failed to publish EngineOutput: {}", e);
                } else {
                    // Update counters
                    zmq_out_cnt.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    zmq_trd_cnt.fetch_add(
                        output.trades.len() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
            }
        }

        // 2. Commit Kafka offset AFTER processing and publishing
        // This ensures at-least-once delivery: if we crash before commit,
        // Kafka will redeliver the message on restart
        if end_of_batch {
            if let Some((topic, partition, offset)) = &event.kafka_offset {
                if let Err(e) = consumer_for_commit.store_offset(topic, *partition, *offset) {
                    eprintln!("Failed to store offset: {}", e);
                }
            }

            // Report ZMQ output throughput every second
            let mut last = zmq_last.lock().unwrap();
            if last.elapsed() >= std::time::Duration::from_secs(1) {
                let total_outputs = zmq_out_cnt.load(std::sync::atomic::Ordering::Relaxed);
                let total_trades = zmq_trd_cnt.load(std::sync::atomic::Ordering::Relaxed);
                let elapsed = zmq_start.lock().unwrap().elapsed().as_secs_f64();
                let ops = if elapsed > 0.0 { total_outputs as f64 / elapsed } else { 0.0 };

                println!(
                    "[ME-OUTPUT] outputs={} trades={} | {:.0} outputs/s",
                    total_outputs, total_trades, ops
                );
                *last = std::time::Instant::now();
            }
        }
    };

    // Build disruptor with 2-Stage Pipeline
    // Stage 1: Matcher processes orders
    // Stage 2: ZMQ Publisher + Offset Commit (AFTER Matcher - safe ordering)
    let mut producer = build_single_producer(8192, factory, BusySpin)
        .handle_events_with(matcher)
        .and_then()
        .handle_events_with(zmq_and_commit)
        .build();

    println!(">>> Disruptor initialized with 2-STAGE PIPELINE:");
    println!(">>> 1. Matcher (processes orders)");
    println!(">>> 2. ZMQ Publisher + Offset Commit (after Matcher)");
    println!(">>> Ring buffer size: 8192, Wait strategy: BusySpin");

    // Main Poll Thread (publishes to disruptor)

    loop {
        let mut batch = Vec::with_capacity(batch_poll_count);

        let poll_start = std::time::Instant::now();

        // 1. Block for at least one message
        match consumer.recv().await {
            Ok(m) => batch.push(m),
            Err(e) => eprintln!("Kafka error: {}", e),
        }
        let wait_time = poll_start.elapsed();

        // 2. Drain whatever else is already in the local buffer (up to 999 more)
        for _ in 0..batch_poll_count - 1 {
            match tokio::time::timeout(std::time::Duration::from_millis(0), consumer.recv()).await {
                Ok(Ok(m)) => batch.push(m),                    // Message received
                Ok(Err(e)) => eprintln!("Kafka error: {}", e), // Kafka error
                Err(_) => break, // Timeout (Buffer empty), stop batching
            }
        }
        let total_poll_time = poll_start.elapsed();

        if batch.is_empty() {
            continue;
        }

        if batch.len() > 0 {
            println!(
                "[PERF] Poll: {} msgs. Wait: {:?}, Drain: {:?}",
                batch.len(),
                wait_time,
                total_poll_time - wait_time
            );
        }

        // println!("Processing batch of {} orders", batch.len());

        // 3. Process the batch (Prepare & Publish to Disruptor)
        let mut place_orders = Vec::with_capacity(batch.len());
        let mut pending_batch_offset: Option<(String, i32, i64)> = None;

        let t_prep_start = std::time::Instant::now();
        for m in batch {
            let topic = m.topic();
            let partition = m.partition();
            let offset = m.offset();
            let timestamp = m.timestamp().to_millis().unwrap_or(0) as u64;
            let current_offset = Some((topic.to_string(), partition, offset));

            if let Some(payload) = m.payload() {
                if topic == config.kafka.topics.orders {
                    // Changed from `order_topic` to `config.kafka.topics.orders` to match existing code
                    // Deserialize Order
                    if let Ok(req) = serde_json::from_slice::<OrderRequest>(payload) {
                        println!("DEBUG: Received OrderRequest: {:?}", req);
                        match req {
                            OrderRequest::PlaceOrder {
                                order_id,
                                user_id,
                                symbol_id,
                                side,
                                price,
                                quantity,
                                order_type,
                                checksum,
                            } => {
                                let calculated = calculate_checksum(
                                    order_id,
                                    user_id,
                                    symbol_id,
                                    side as u8,
                                    price,
                                    quantity,
                                    order_type as u8,
                                );
                                if calculated != checksum {
                                    eprintln!("❌ Checksum Mismatch! OrderID: {}", order_id);
                                    continue;
                                }
                                if let Some(_symbol_name) = symbol_manager.get_symbol(symbol_id) {
                                    // Accumulate orders for batch processing
                                    place_orders.push((
                                        symbol_id, order_id, side, order_type, price, quantity,
                                        user_id, timestamp,
                                    ));
                                    pending_batch_offset = current_offset;
                                } else {
                                    eprintln!("Unknown symbol ID: {}", symbol_id);
                                }
                            }
                            OrderRequest::CancelOrder { order_id, symbol_id, .. } => {
                                if let Some(_symbol_name) = symbol_manager.get_symbol(symbol_id) {
                                    // Publish pending place orders first
                                    if !place_orders.is_empty() {
                                        let count = place_orders.len();
                                        let batch_clone = place_orders.clone();
                                        let offset_clone = pending_batch_offset.clone();
                                        println!(
                                            "[Poll] Publishing batch of {} orders before cancel",
                                            count
                                        );
                                        producer.publish(|event| {
                                            event.command =
                                                Some(EngineCommand::PlaceOrderBatch(batch_clone));
                                            event.kafka_offset = offset_clone;
                                            event.timestamp = timestamp; // Use latest timestamp for batch event metadata
                                        });
                                        place_orders.clear();
                                        pending_batch_offset = None;
                                    }

                                    // Publish cancel order
                                    println!("[Poll] Publishing cancel order {}", order_id);
                                    let offset_clone = current_offset.clone();
                                    producer.publish(|event| {
                                        event.command = Some(EngineCommand::CancelOrder {
                                            symbol_id,
                                            order_id,
                                        });
                                        event.kafka_offset = offset_clone;
                                        event.timestamp = timestamp;
                                    });
                                } else {
                                    eprintln!("Unknown symbol ID: {}", symbol_id);
                                }
                            }
                        }
                    } else {
                        eprintln!("Failed to parse Order JSON");
                    }
                } /* REMOVED: Balance topic handling - balance operations now in UBSCore
                else if topic == balance_topic {
                    // Deserialize Balance Request
                    match serde_json::from_slice::<BalanceRequest>(payload) {
                        ... removed
                    }
                }
                */
            }
        }

        // End of batch: Publish remaining place orders
        if !place_orders.is_empty() {
            let count = place_orders.len();
            let batch_clone = place_orders.clone();
            let offset_clone = pending_batch_offset.clone();
            println!("[Poll] Publishing final batch of {} orders", count);
            // Use last seen timestamp for final batch? Or 0?
            // We don't have 'timestamp' here easily unless we tracked it.
            // But place_orders has timestamps inside.
            // The event.timestamp is less important for batch.
            producer.publish(|event| {
                event.command = Some(EngineCommand::PlaceOrderBatch(batch_clone));
                event.kafka_offset = offset_clone;
                event.timestamp = 0; // Placeholder
            });

            total_orders += count;
            if last_report.elapsed() >= std::time::Duration::from_secs(5) {
                let elapsed = last_report.elapsed().as_secs_f64();
                let ops = total_orders as f64 / elapsed;
                println!("[PERF] OPS: {:.2}, Last Batch: {}", ops, count);
                total_orders = 0;
                last_report = std::time::Instant::now();
            }
        }

        let t_prep = t_prep_start.elapsed();

        /*
        if batch_len > 0 {
            println!(
                "[PERF] Loop Active: {:?}. Prep: {:?}, Engine: (Pipelined)",
                poll_start.elapsed() - wait_time,
                t_prep
            );
        }
        */
    }
}
