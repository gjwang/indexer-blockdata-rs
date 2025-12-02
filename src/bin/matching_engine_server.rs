use std::fs;
use std::path::Path;

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};

use fetcher::ledger::{LedgerCommand, LedgerListener};
use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::OrderRequest;
use fetcher::symbol_manager::SymbolManager;

struct RedpandaTradeProducer {
    producer: FutureProducer,
    topic: String,
}

impl LedgerListener for RedpandaTradeProducer {
    fn on_command(&mut self, cmd: &LedgerCommand) -> Result<(), anyhow::Error> {
        if let LedgerCommand::MatchExecBatch(batch) = cmd {
            let producer = self.producer.clone();
            let topic = self.topic.clone();
            let batch = batch.clone();

            tokio::spawn(async move {
                if batch.is_empty() {
                    return;
                }

                // 1. Convert to Trade Models
                let trades: Vec<fetcher::models::Trade> = batch
                    .iter()
                    .map(|data| fetcher::models::Trade {
                        trade_id: data.trade_id,
                        buy_order_id: data.buy_order_id,
                        sell_order_id: data.sell_order_id,
                        buy_user_id: data.buyer_user_id,
                        sell_user_id: data.seller_user_id,
                        price: data.price,
                        quantity: data.quantity,
                        match_seq: data.match_seq,
                    })
                    .collect();

                // 2. Determine Key (Symbol Pair) to ensure ordering
                let first = &batch[0];
                let key = format!("{}_{}", first.base_asset, first.quote_asset);
                // Debug: log number of trades being sent
                println!("Sending {} trades to topic '{}' with key '{}'", trades.len(), topic, key);

                // 3. Serialize and Send Batch
                if let Ok(payload) = serde_json::to_string(&trades) {
                    match producer
                        .send(
                            FutureRecord::to(&topic).payload(&payload).key(&key),
                            std::time::Duration::from_secs(0),
                        )
                        .await
                    {
                        Ok(_) => println!("Trades batch sent successfully to topic '{}'", topic),
                        Err((e, _)) => eprintln!("Failed to send trades batch to topic '{}' with key '{}': {}", topic, key, e),
                    };
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

    consumer
        .subscribe(&[&config.kafka.topics.orders])
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

    loop {
        let mut batch = Vec::with_capacity(1000);

        // 1. Block for at least one message
        match consumer.recv().await {
            Ok(m) => batch.push(m),
            Err(e) => eprintln!("Kafka error: {}", e),
        }

        // 2. Drain whatever else is already in the local buffer (up to 999 more)
        for _ in 0..999 {
            match tokio::time::timeout(std::time::Duration::from_millis(0), consumer.recv()).await {
                Ok(Ok(m)) => batch.push(m), // Message received
                Ok(Err(e)) => eprintln!("Kafka error: {}", e), // Kafka error
                Err(_) => break, // Timeout (Buffer empty), stop batching
            }
        }

        if batch.is_empty() {
            continue;
        }

        println!("Processing batch of {} orders", batch.len());

        // 3. Process the batch
        let mut place_orders = Vec::with_capacity(batch.len());

        for m in batch {
            if let Some(payload) = m.payload_view::<str>() {
                match payload {
                    Ok(text) => {
                        // Deserialize
                        if let Ok(req) = serde_json::from_str::<OrderRequest>(text) {
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
                                        place_orders.push((symbol_id, order_id, side, order_type, price, quantity, user_id));
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
                            eprintln!("Failed to parse JSON: {}", text);
                        }
                    }
                    Err(e) => eprintln!("Error reading payload: {}", e),
                }
            }
        }

        if !place_orders.is_empty() {
            let count = place_orders.len();
            let results = engine.add_order_batch(place_orders);
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
    }
}

