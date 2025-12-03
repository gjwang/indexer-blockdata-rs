use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};

use disruptor::*;

use fetcher::ledger::{LedgerCommand, LedgerListener, MatchExecData};
use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::{BalanceRequest, OrderRequest, OrderType, Side};
use fetcher::symbol_manager::SymbolManager;

/// Event structure for the Disruptor ring buffer
struct OrderEvent {
    command: Option<EngineCommand>,
    processing_result: std::sync::Mutex<Option<std::sync::Arc<Vec<LedgerCommand>>>>,
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

struct BalanceProcessor {
    recent_requests: HashMap<String, u64>,
    request_queue: VecDeque<(String, u64)>,
}

impl BalanceProcessor {
    fn new() -> Self {
        Self {
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
            } else {
                break;
            }
        }
    }

    fn process_balance_request(
        &mut self,
        engine: &mut MatchingEngine,
        req: BalanceRequest,
    ) -> Result<(), anyhow::Error> {
        let current_time = self.current_time_ms();
        let request_id = req.request_id().to_string();

        // 1. Validate timestamp
        if !req.is_within_time_window(current_time) {
            println!("❌ REJECTED: Request outside time window: {}", request_id);
            return Ok(());
        }

        // 2. Check duplicate
        if self.recent_requests.contains_key(&request_id) {
            println!("❌ REJECTED: Duplicate request: {}", request_id);
            return Ok(());
        }

        // 3. Process
        match req {
            BalanceRequest::TransferIn {
                user_id,
                asset_id,
                amount,
                timestamp,
                ..
            } => {
                println!(
                    "📥 Transfer In: {} asset {} -> user {}",
                    amount, asset_id, user_id
                );

                // Direct call, no lock needed!
                match engine.transfer_in_to_trading_account(user_id, asset_id, amount) {
                    Ok(()) => {
                        println!("✅ Transfer In success: {}", request_id);
                        self.recent_requests.insert(request_id.clone(), timestamp);
                        self.request_queue.push_back((request_id, timestamp));
                    }
                    Err(e) => {
                        println!("❌ Transfer In failed: {}", e);
                    }
                }
            }
            BalanceRequest::TransferOut {
                user_id,
                asset_id,
                amount,
                timestamp,
                ..
            } => {
                println!(
                    "📤 Transfer Out: {} asset {} <- user {}",
                    amount, asset_id, user_id
                );
                // Direct call, no lock needed!
                match engine.transfer_out_from_trading_account(user_id, asset_id, amount) {
                    Ok(()) => {
                        println!("✅ Transfer Out success: {}", request_id);
                        self.recent_requests.insert(request_id.clone(), timestamp);
                        self.request_queue.push_back((request_id, timestamp));
                    }
                    Err(e) => println!("❌ Transfer Out failed: {}", e),
                }
            }
        }
        self.cleanup_old_requests();
        Ok(())
    }
}

struct RedpandaTradeProducer {
    producer: FutureProducer,
    topic: String,
    runtime_handle: tokio::runtime::Handle,
}

impl RedpandaTradeProducer {
    fn new(producer: FutureProducer, topic: String, runtime_handle: tokio::runtime::Handle) -> Self {
        Self { producer, topic, runtime_handle }
    }

    fn collect_trades(cmd: &LedgerCommand, trades: &mut Vec<fetcher::models::Trade>) {
        match cmd {
            LedgerCommand::MatchExec(data) => {
                trades.push(Self::to_trade(data));
            }
            LedgerCommand::MatchExecBatch(batch) => {
                for data in batch {
                    trades.push(Self::to_trade(data));
                }
            }
            LedgerCommand::Batch(cmds) => {
                for c in cmds {
                    Self::collect_trades(c, trades);
                }
            }
            _ => {}
        }
    }

    fn to_trade(data: &MatchExecData) -> fetcher::models::Trade {
        fetcher::models::Trade {
            trade_id: data.trade_id,
            buy_order_id: data.buy_order_id,
            sell_order_id: data.sell_order_id,
            buy_user_id: data.buyer_user_id,
            sell_user_id: data.seller_user_id,
            price: data.price,
            quantity: data.quantity,
            match_seq: data.match_seq,
        }
    }
}

impl LedgerListener for RedpandaTradeProducer {
    fn on_command(&mut self, _cmd: &LedgerCommand) -> Result<(), anyhow::Error> {
        Ok(())
    }

    fn on_batch(&mut self, cmds: &[LedgerCommand]) -> Result<(), anyhow::Error> {
        // Collect all trades from the batch
        let mut all_trades = Vec::new();
        for cmd in cmds {
            Self::collect_trades(cmd, &mut all_trades);
        }

        if all_trades.is_empty() {
            return Ok(());
        }

        // Send trades to Kafka using the runtime handle
        if let Ok(payload) = serde_json::to_vec(&all_trades) {
            let key = "batch";
            let producer = self.producer.clone();
            let topic = self.topic.clone();
            let count = all_trades.len();
            
            // Use the runtime handle to SPAWN the task (Fire-and-Forget)
            // This returns immediately and doesn't block the matcher thread
            self.runtime_handle.spawn(async move {
                match producer.send(
                    FutureRecord::to(&topic).payload(&payload).key(key),
                    std::time::Duration::from_secs(0), // No wait for queue full
                ).await {
                    Ok(_) => {
                        // println!("[Trade Publisher] Published {} trades to {}", count, topic);
                    }
                    Err((e, _)) => {
                        eprintln!("[Trade Publisher] Failed to send trades: {}", e);
                    }
                }
            });
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");
    let wal_dir = Path::new("me_wal_data");
    let snap_dir = Path::new("me_snapshots");

    // Clean up previous run (Optional: maybe we want to recover?)
    // For this demo refactor, let's keep it clean to avoid state issues during dev.
    if wal_dir.exists() {
        let _ = fs::remove_dir_all(wal_dir);
    }
    if snap_dir.exists() {
        let _ = fs::remove_dir_all(snap_dir);
    }

    let mut engine = MatchingEngine::new(wal_dir, snap_dir).expect("Failed to create engine");

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
        engine
            .register_symbol(symbol_id, symbol.clone(), base, quote)
            .unwrap();
        println!("Loaded symbol: {}", symbol);
    }

    // Deposit Funds for generic users
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
        .set(
            "socket.keepalive.enable",
            &config.kafka.socket_keepalive_enable,
        )
        .create()
        .expect("Consumer creation failed");
    let consumer = std::sync::Arc::new(consumer_raw);

    // === Kafka Producer Setup ===
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Producer creation failed");

    // Trade producer will be used in Output Processor
    let mut trade_producer = RedpandaTradeProducer::new(
        producer.clone(),
        config.kafka.topics.trades.clone(),
        tokio::runtime::Handle::current(),
    );

    let balance_topic = config
        .kafka
        .topics
        .balance_ops
        .clone()
        .unwrap_or("balance.operations".to_string());
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
    println!("  WAL Directory:     {:?}", wal_dir);
    println!("  Snapshot Dir:      {:?}", snap_dir);
    println!("--------------------------------------------------");
    println!(">>> Matching Engine Server Started (Pipelined)");

    let mut total_orders = 0;
    let mut last_report = std::time::Instant::now();

    let batch_poll_count = 1000;

    // === True Pipeline Architecture with Disruptor ===
    // Extract OrderWal from Engine to give to WAL Writer thread
    let mut order_wal = engine.take_order_wal().expect("OrderWAL not found");
    let progress_handle = order_wal.get_progress_handle();

    // Extract Ledger WAL for Output Processor (Consumer 3)
    let mut ledger_wal = engine.ledger.take_wal().expect("Ledger WAL not found");
    let mut last_ledger_seq = engine.ledger.last_seq;
    
    // Factory: Create empty events in the ring buffer
    let factory = || OrderEvent { command: None, processing_result: std::sync::Mutex::new(None), kafka_offset: None, timestamp: 0 };
    
    // === Consumer 1: WAL Writer (writes to WAL, updates progress) ===
    let progress_for_writer = progress_handle.clone();
    let mut batch_start = std::time::Instant::now();
    let mut batch_count = 0;
    let consumer_for_writer = consumer.clone();

    let wal_writer = move |event: &OrderEvent, sequence: Sequence, end_of_batch: bool| {
        if batch_count == 0 {
            batch_start = std::time::Instant::now();
        }
        batch_count += 1;

        if let Some(ref cmd) = event.command {
            match cmd {
                EngineCommand::PlaceOrderBatch(batch) => {
                    for (symbol_id, order_id, side, _order_type, price, quantity, user_id, _timestamp) in batch {
                        let wal_side = *side;
                        if let Err(e) = order_wal.log_place_order_no_flush(
                            *order_id, *user_id, *symbol_id, wal_side, *price, *quantity,
                        ) {
                            panic!("CRITICAL: Failed to write to Order WAL: {}. Halting.", e);
                        }
                    }
                    order_wal.current_seq = sequence as u64;
                }
                EngineCommand::CancelOrder { order_id, .. } => {
                    if let Err(e) = order_wal.log_cancel_order_no_flush(*order_id) {
                        panic!("CRITICAL: Failed to write Cancel to Order WAL: {}. Halting.", e);
                    }
                    order_wal.current_seq = sequence as u64;
                }
                EngineCommand::BalanceRequest(_) => {
                    order_wal.current_seq = sequence as u64;
                }
            }
        }
        
        // Flush WAL at end of batch and update progress
        if end_of_batch {
            let start_flush = std::time::Instant::now();
            if let Err(e) = order_wal.flush() {
                panic!("CRITICAL: Failed to flush Order WAL: {}. Halting.", e);
            }
            let flush_time = start_flush.elapsed();
            
            // Only print if there was actual work (batch_count > 0)
            if batch_count > 0 {
                 println!("[PERF] WAL Writer: {} events. Total: {:?}, Flush: {:?}", 
                     batch_count, batch_start.elapsed(), flush_time);
            }
            batch_count = 0;

            // Commit Offset (if any) - Only after successful flush!
            if let Some((ref topic, partition, offset)) = event.kafka_offset {
                 let mut tpl = rdkafka::TopicPartitionList::new();
                 let _ = tpl.add_partition_offset(topic, partition, rdkafka::Offset::Offset(offset + 1));
                 if let Err(e) = consumer_for_writer.store_offsets(&tpl) {
                      eprintln!("Offset Store Error: {}", e);
                 }
            }
            // Progress is updated inside flush() (Wait, flush just flushes disk)
            // We need to ensure progress is updated?
            // Ah, order_wal.flush() doesn't update progress_handle?
            // Let's check order_wal.
        }
    };
    
    // === Consumer 2: Matcher (waits for progress, matches, updates memory) ===
    let progress_for_matcher = progress_handle.clone();
    let matcher = move |event: &OrderEvent, sequence: Sequence, _end_of_batch: bool| {
        // Wait until this sequence is persisted in WAL
        let mut spins = 0;
        loop {
            let progress = progress_for_matcher.load(std::sync::atomic::Ordering::Acquire);
            if (sequence as u64) <= progress {
                break; // Safe to process
            }
            
            if spins < 10000 {
                std::hint::spin_loop();
                spins += 1;
            } else {
                std::thread::yield_now();
            }
        }
        
        // Now safe to process - WAL is guaranteed to be flushed
        if let Some(ref cmd) = event.command {
            match cmd {
                EngineCommand::PlaceOrderBatch(batch) => {
                    let (_, cmds) = engine.add_order_batch(batch.clone());
                    // Pass commands to Output Processor via Event
                    *event.processing_result.lock().unwrap() = Some(std::sync::Arc::new(cmds));
                }
                EngineCommand::CancelOrder { symbol_id, order_id } => {
                    let _ = engine.cancel_order(*symbol_id, *order_id);
                }
                EngineCommand::BalanceRequest(req) => {
                    if let Err(e) = balance_processor.process_balance_request(&mut engine, req.clone()) {
                        eprintln!("Balance processing error: {}", e);
                    }
                }
            }
        }
    };

    // === Consumer 3a: Ledger Writer (Writes to Ledger WAL) ===
    let ledger_writer = move |event: &OrderEvent, _sequence: Sequence, _end_of_batch: bool| {
        let cmds_arc = {
            event.processing_result.lock().unwrap().as_ref().cloned()
        };
        if let Some(cmds) = cmds_arc {
            if !cmds.is_empty() {
                // Write to Ledger WAL
                for cmd in cmds.iter() {
                    last_ledger_seq += 1;
                    if let Err(e) = ledger_wal.append_no_flush(last_ledger_seq, cmd) {
                        panic!("CRITICAL: Failed to write to Ledger WAL: {}. Halting.", e);
                    }
                }
            }
        }
    };

    // === Consumer 3b: Trade Publisher (Publishes Trades to Kafka) ===
    let trade_publisher = move |event: &OrderEvent, _sequence: Sequence, _end_of_batch: bool| {
        let cmds_arc = {
            event.processing_result.lock().unwrap().as_ref().cloned()
        };
        if let Some(cmds) = cmds_arc {
            if !cmds.is_empty() {
                let start = std::time::Instant::now();
                if let Err(e) = trade_producer.on_batch(&cmds) {
                     eprintln!("Trade Publish Error: {}", e);
                }
                println!("[PERF] Output(Trade): {} cmds. Time: {:?}", cmds.len(), start.elapsed());
            }
        }
    };
    
    // Build disruptor with 3-Stage Pipeline (Stage 3 is Parallel)
    // 1. WAL Writer & Matcher run in parallel
    // 2. Ledger Writer & Trade Publisher run in parallel AFTER Matcher
    let mut producer = build_single_producer(8192, factory, BusySpin)
        .handle_events_with(wal_writer)
        .handle_events_with(matcher)
        .and_then()
        .handle_events_with(ledger_writer)
        .handle_events_with(trade_publisher)
        .build();
    
    println!(">>> Disruptor initialized with 3-STAGE PIPELINE (Parallel Output):");
    println!(">>> 1. WAL Writer (Parallel)");
    println!(">>> 2. Matcher (Parallel, gated on Writer)");
    println!(">>> 3. Ledger Writer & Trade Publisher (Parallel after Matcher)");
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
                if topic == config.kafka.topics.orders { // Changed from `order_topic` to `config.kafka.topics.orders` to match existing code
                    // Deserialize Order
                    if let Ok(req) = serde_json::from_slice::<OrderRequest>(payload) {
                        match req {
                            OrderRequest::PlaceOrder {
                                order_id,
                                user_id,
                                symbol_id,
                                side,
                                price,
                                quantity,
                                order_type,
                            } => {
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
                            OrderRequest::CancelOrder {
                                order_id,
                                symbol_id,
                                ..
                            } => {
                                if let Some(_symbol_name) = symbol_manager.get_symbol(symbol_id) {
                                    // Publish pending place orders first
                                    if !place_orders.is_empty() {
                                        let count = place_orders.len();
                                        let batch_clone = place_orders.clone();
                                        let offset_clone = pending_batch_offset.clone();
                                        println!("[Poll] Publishing batch of {} orders before cancel", count);
                                        producer.publish(|event| {
                                            event.command = Some(EngineCommand::PlaceOrderBatch(batch_clone));
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
                    if let Ok(req) = serde_json::from_slice::<BalanceRequest>(payload) {
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
                    } else {
                        eprintln!("Failed to parse Balance JSON");
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
