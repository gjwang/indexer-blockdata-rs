use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};

use disruptor::*;

use fetcher::ledger::LedgerCommand;
use fetcher::engine_output::EngineOutput;
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
/// Max order IDs to track for deduplication
const MAX_TRACKED_ORDERS: usize = 100_000;

/// Tracks recently processed order IDs to detect duplicates
/// Critical for at-least-once delivery: Kafka may redeliver on crash recovery
struct OrderDeduplicator {
    recent_orders: std::collections::HashSet<u64>,
    order_queue: VecDeque<u64>,
}

impl OrderDeduplicator {
    fn new() -> Self {
        Self {
            recent_orders: std::collections::HashSet::with_capacity(MAX_TRACKED_ORDERS),
            order_queue: VecDeque::with_capacity(MAX_TRACKED_ORDERS),
        }
    }

    /// Check if order was already processed. Returns true if duplicate.
    fn is_duplicate(&self, order_id: u64) -> bool {
        self.recent_orders.contains(&order_id)
    }

    /// Mark order as processed
    fn mark_processed(&mut self, order_id: u64) {
        if self.recent_orders.insert(order_id) {
            self.order_queue.push_back(order_id);

            // Evict oldest if over limit
            while self.order_queue.len() > MAX_TRACKED_ORDERS {
                if let Some(oldest) = self.order_queue.pop_front() {
                    self.recent_orders.remove(&oldest);
                }
            }
        }
    }
}

struct BalanceProcessor {
    recent_requests: HashMap<String, u64>,
    request_queue: VecDeque<(String, u64)>,
}

impl BalanceProcessor {
    fn new() -> Self {
        Self { recent_requests: HashMap::new(), request_queue: VecDeque::new() }
    }

    fn current_time_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
    }

    fn cleanup_old_requests(&mut self) {
        let current_time = self.current_time_ms();
        while let Some((request_id, timestamp)) = self.request_queue.front().cloned() {
            if current_time - timestamp > TRACKING_WINDOW_MS {
                self.recent_requests.remove(&request_id);
                self.request_queue.pop_front();
            } else {
                break;
            }
        }
    }

    fn process_balance_request(
        &mut self,
        engine: &mut MatchingEngine,
        req: BalanceRequest,
    ) -> Result<Vec<EngineOutput>, anyhow::Error> {
        let current_time = self.current_time_ms();
        let request_id = req.request_id().to_string();
        let mut outputs = Vec::new();

        // 1. Validate timestamp
        if !req.is_within_time_window(current_time) {
            println!("❌ REJECTED: Request outside time window: {}", request_id);
            return Ok(outputs);
        }

        // 2. Check duplicate
        if self.recent_requests.contains_key(&request_id) {
            println!("❌ REJECTED: Duplicate request: {}", request_id);
            return Ok(outputs);
        }

        // 3. Process
        match req {
            BalanceRequest::TransferIn { user_id, asset_id, amount, timestamp, .. } => {
                println!("📥 Transfer In: {} asset {} -> user {}", amount, asset_id, user_id);

                match engine.transfer_in_and_build_output(user_id, asset_id, amount) {
                    Ok(output) => {
                        println!("✅ Transfer In success: {}", request_id);
                        self.recent_requests.insert(request_id.clone(), timestamp);
                        self.request_queue.push_back((request_id, timestamp));
                        outputs.push(output);
                    }
                    Err(e) => {
                        println!("❌ Transfer In failed: {}", e);
                    }
                }
            }
            BalanceRequest::TransferOut { user_id, asset_id, amount, timestamp, .. } => {
                println!("📤 Transfer Out: {} asset {} <- user {}", amount, asset_id, user_id);

                match engine.transfer_out_and_build_output(user_id, asset_id, amount) {
                    Ok(output) => {
                        println!("✅ Transfer Out success: {}", request_id);
                        self.recent_requests.insert(request_id.clone(), timestamp);
                        self.request_queue.push_back((request_id, timestamp));
                        outputs.push(output);
                    }
                    Err(e) => println!("❌ Transfer Out failed: {}", e),
                }
            }
        }
        self.cleanup_old_requests();
        Ok(outputs)
    }
}



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
    let mut engine = MatchingEngine::new(dummy_wal_dir, snap_dir, false)
        .expect("Failed to create engine");

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

    // NOTE: Automatic deposits disabled - use transfer_in API for proper E2E flow
    // Users must call /api/v1/transfer_in to initialize their balances
    // This ensures the full flow: transfer_in -> deposit event -> settlement -> user_balances
    /*
    println!("=== Depositing Funds ===");
    let amount = 100_000_000_u64;

    for uid in 0..5000 {
        for asset_id in [1, 2, 3] {
            // BTC, USDT, ETH
            let decimal = symbol_manager.get_asset_decimal(asset_id).unwrap_or(8);
            let amount_raw = amount * 10_u64.pow(decimal);

            engine
                .ledger
                .apply(&LedgerCommand::Deposit {
                    user_id: uid,
                    asset: asset_id,
                    amount: amount_raw,
                })
                .unwrap();
        }
    }
    println!("Funds deposited for users 0-5000.");
    */

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


    let balance_topic =
        config.kafka.topics.balance_ops.clone().unwrap_or("balance.operations".to_string());
    let mut balance_processor = BalanceProcessor::new();

    consumer
        .subscribe(&[&config.kafka.topics.orders, &balance_topic])
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
    sync_req.connect(&format!("tcp://localhost:{}", sync_port)).expect("Failed to connect to sync port");
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
    // Order deduplicator for at-least-once delivery safety
    let mut order_dedup = OrderDeduplicator::new();

    // Factory: Create empty events in the ring buffer
    let factory = || OrderEvent {
        command: None,
        engine_outputs: std::sync::Mutex::new(None),
        kafka_offset: None,
        timestamp: 0,
    };

    // === Consumer 1: Matcher (processes orders and produces EngineOutput) ===
    let matcher = move |event: &OrderEvent, _sequence: Sequence, _end_of_batch: bool| {
        // Process commands and produce EngineOutput bundles
        if let Some(ref cmd) = event.command {
            match cmd {
                EngineCommand::PlaceOrderBatch(batch) => {
                    // Process each order individually, producing EngineOutput bundles
                    // Each output is chain-linked to the previous for integrity verification
                    // IMPORTANT: Check for duplicates to handle Kafka redelivery safely
                    let mut engine_outputs = Vec::with_capacity(batch.len());
                    let mut input_seq = engine.get_output_seq();

                    for (symbol_id, order_id, side, order_type, price, quantity, user_id, timestamp) in batch.iter() {
                        // Check for duplicate order (Kafka may redeliver after crash)
                        if order_dedup.is_duplicate(*order_id) {
                            println!("[ME] DUPLICATE ORDER {} - skipping (already processed)", order_id);
                            continue;
                        }

                        input_seq += 1;
                        let cid = format!("kafka_{}", order_id);

                        match engine.add_order_and_build_output(
                            input_seq,
                            *symbol_id,
                            *order_id,
                            *side,
                            *order_type,
                            *price,
                            *quantity,
                            *user_id,
                            *timestamp,
                            cid,
                        ) {
                            Ok((_, output)) => {
                                // Mark as processed AFTER successful execution
                                order_dedup.mark_processed(*order_id);
                                engine_outputs.push(output);
                            }
                            Err(e) => {
                                eprintln!("[ME] EngineOutput error for order {}: {:?}", order_id, e);
                            }
                        }
                    }

                    if !engine_outputs.is_empty() {
                        *event.engine_outputs.lock().unwrap() = Some(std::sync::Arc::new(engine_outputs));
                    }
                }
                EngineCommand::CancelOrder { symbol_id, order_id } => {
                    let _ = engine.cancel_order(*symbol_id, *order_id);
                }
                EngineCommand::BalanceRequest(req) => {
                    // Balance requests now use EngineOutput flow
                    match balance_processor.process_balance_request(&mut engine, req.clone()) {
                        Ok(outputs) => {
                            if !outputs.is_empty() {
                                // Append to existing engine_outputs or create new
                                let mut guard = event.engine_outputs.lock().unwrap();
                                if let Some(ref mut existing) = *guard {
                                    // In practice, balance requests are processed separately
                                    // so this shouldn't happen, but handle it gracefully
                                    let mut combined = (*existing).as_ref().clone();
                                    combined.extend(outputs);
                                    *guard = Some(std::sync::Arc::new(combined));
                                } else {
                                    *guard = Some(std::sync::Arc::new(outputs));
                                }
                            }
                        }
                        Err(e) => eprintln!("Balance processing error: {}", e),
                    }
                }
            }
        }
    };

    // === Consumer 2: ZMQ Publisher + Offset Committer (AFTER Matcher) ===
    // CRITICAL: Offset must be committed AFTER processing to prevent message loss
    let zmq_pub_clone = zmq_publisher.clone();
    let consumer_for_commit = consumer.clone();
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
                    println!("[ZMQ] Published EngineOutput seq={} trades={} balance_events={}",
                        output.output_seq, output.trades.len(), output.balance_events.len());
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
                } else if topic == balance_topic {
                    // Deserialize Balance Request
                    match serde_json::from_slice::<BalanceRequest>(payload) {
                        Ok(req) => {
                            // Publish pending place orders first
                            if !place_orders.is_empty() {
                                let batch_clone = place_orders.clone();
                                let offset_clone = pending_batch_offset.clone();
                                producer.publish(|event| {
                                    event.command = Some(EngineCommand::PlaceOrderBatch(batch_clone));
                                    event.kafka_offset = offset_clone;
                                    event.timestamp = timestamp;
                                });
                                place_orders.clear();
                                pending_batch_offset = None;
                            }

                            // Publish balance request
                            let offset_clone = current_offset.clone();
                            producer.publish(|event| {
                                event.command = Some(EngineCommand::BalanceRequest(req));
                                event.kafka_offset = offset_clone;
                                event.timestamp = timestamp;
                            });
                        }
                        Err(e) => {
                            eprintln!("Failed to parse Balance JSON: {}", e);
                            eprintln!("Payload: {}", String::from_utf8_lossy(payload));
                        }
                    }
                }
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
