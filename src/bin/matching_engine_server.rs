use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};

use fetcher::ledger::{LedgerCommand, LedgerListener, MatchExecData};
use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::BalanceRequest;
use fetcher::models::OrderRequest;
use fetcher::symbol_manager::SymbolManager;

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
}

impl RedpandaTradeProducer {
    fn collect_trades(cmd: &LedgerCommand, buffer: &mut Vec<fetcher::models::Trade>) {
        match cmd {
            LedgerCommand::MatchExec(data) => {
                buffer.push(Self::to_trade(data));
            }
            LedgerCommand::MatchExecBatch(batch) => {
                for data in batch {
                    buffer.push(Self::to_trade(data));
                }
            }
            LedgerCommand::Batch(cmds) => {
                for c in cmds {
                    Self::collect_trades(c, buffer);
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
    fn on_command(&mut self, cmd: &LedgerCommand) -> Result<(), anyhow::Error> {
        // Delegate to on_batch for consistency, or keep as is?
        // Let's wrap it in a slice and call on_batch to reuse logic
        self.on_batch(std::slice::from_ref(cmd))
    }

    fn on_batch(&mut self, cmds: &[LedgerCommand]) -> Result<(), anyhow::Error> {


        // Clone commands to move them to the background task
        let cmds = cmds.to_vec();
        let producer = self.producer.clone();
        let topic = self.topic.clone();

        tokio::spawn(async move {
            let mut all_trades = Vec::new();
            for cmd in &cmds {
                Self::collect_trades(cmd, &mut all_trades);
            }

            if all_trades.is_empty() {
                return;
            }

            if let Ok(payload) = serde_json::to_vec(&all_trades) {
                let key = "batch";
                match producer
                    .send(
                        FutureRecord::to(&topic).payload(&payload).key(key),
                        std::time::Duration::from_secs(0),
                    )
                    .await
                {
                    Ok(_) => {
                        // println!("Trades batch sent successfully to topic '{}'", topic)
                    }
                    Err((e, _)) => {
                        eprintln!("Failed to send trades batch to topic '{}': {}", topic, e)
                    }
                };
            }
        });
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
    // === Kafka Consumer Setup ===
    let consumer: StreamConsumer = ClientConfig::new()
        .set("group.id", &config.kafka.group_id)
        .set("bootstrap.servers", &config.kafka.broker)
        .set("auto.offset.reset", "earliest")
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

    // === Kafka Producer Setup ===
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Producer creation failed");

    let trade_producer = RedpandaTradeProducer {
        producer: producer.clone(), // Clone producer for trade_producer
        topic: config.kafka.topics.trades.clone(),
    };
    engine.ledger.set_listener(Box::new(trade_producer));

    let balance_topic = config
        .kafka
        .topics
        .balance_ops
        .clone()
        .unwrap_or("balance.operations".to_string());
    let mut balance_processor = BalanceProcessor::new();

    consumer
        .subscribe(&[&config.kafka.topics.orders, &balance_topic])
        .expect("Can't subscribe");

    println!("--------------------------------------------------");
    println!("Boot Parameters:");
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Orders Topic:      {}", config.kafka.topics.orders);
    println!("  Trades Topic:      {}", config.kafka.topics.trades);
    println!("  Consumer Group:    {}", config.kafka.group_id);
    println!("  WAL Directory:     {:?}", wal_dir);
    println!("  Snapshot Dir:      {:?}", snap_dir);
    println!("--------------------------------------------------");
    println!(">>> Matching Engine Server Started");

    let mut total_orders = 0;
    let mut last_report = std::time::Instant::now();

    let batch_poll_count = 1000;

    loop {
        //TODO: review architecture of this loop
        //we already order store in the redpanda,
        //Do we need to duplicate write order wal?
        //My Q: if something bad happen,
        // How do we base on ledger and redpanda, no order_wal

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

        println!("Processing batch of {} orders", batch.len());

        // 3. Process the batch
        let batch_len = batch.len();
        let mut place_orders = Vec::with_capacity(batch.len());

        let t_prep_start = std::time::Instant::now();
        for m in batch {
            let topic = m.topic();
            if let Some(payload) = m.payload() {
                if topic == config.kafka.topics.orders {
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
                                    place_orders.push((
                                        symbol_id, order_id, side, order_type, price, quantity,
                                        user_id,
                                    ));
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
                                    match engine.cancel_order(symbol_id, order_id) {
                                        Ok(_) => println!("Order {} Cancelled", order_id),
                                        Err(e) => eprintln!("Cancel {} Failed: {}", order_id, e),
                                    }
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
                        if let Err(e) = balance_processor.process_balance_request(&mut engine, req)
                        {
                            eprintln!("Balance processing error: {}", e);
                        }
                    } else {
                        eprintln!("Failed to parse Balance JSON");
                    }
                }
            }
        }
        let t_prep = t_prep_start.elapsed();
        let mut t_engine = std::time::Duration::from_secs(0);

        if !place_orders.is_empty() {
            let count = place_orders.len();
            let t_engine_start = std::time::Instant::now();
            let results = engine.add_order_batch(place_orders);
            t_engine = t_engine_start.elapsed();
            for res in results {
                if let Err(e) = res {
                    eprintln!("Failed to add order in batch: {}", e);
                }
            }

            total_orders += count;
            if last_report.elapsed() >= std::time::Duration::from_secs(5) {
                let elapsed = last_report.elapsed().as_secs_f64();
                let ops = total_orders as f64 / elapsed;
                println!("[PERF] OPS: {:.2}, Last Batch: {}", ops, count);
                total_orders = 0;
                last_report = std::time::Instant::now();
            }
        }

        if batch_len > 0 {
            println!(
                "[PERF] Loop Active: {:?}. Prep: {:?}, Engine: {:?}",
                poll_start.elapsed() - wait_time,
                t_prep,
                t_engine
            );
        }
    }
}
