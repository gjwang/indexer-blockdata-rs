use fetcher::ledger::LedgerCommand;
use fetcher::matching_engine_base::{MatchingEngine, SymbolManager};
use fetcher::models::{OrderRequest, OrderType, Side};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use std::fs;
use std::path::Path;

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
        engine.register_symbol(symbol_id, symbol.clone(), base, quote).unwrap();
        println!("Loaded symbol: {}", symbol);
    }

    // Deposit Funds for generic users (1000-1100 range used by gateway)
    // Let's just give a lot of funds to user 1000-1100
    for uid in 1000..1100 {
         engine.ledger.apply(&LedgerCommand::Deposit { user_id: uid, asset: 1, amount: 1_000_000_000 }).unwrap(); // BTC
         engine.ledger.apply(&LedgerCommand::Deposit { user_id: uid, asset: 2, amount: 1_000_000_000 }).unwrap(); // USDT
         engine.ledger.apply(&LedgerCommand::Deposit { user_id: uid, asset: 3, amount: 1_000_000_000 }).unwrap(); // ETH
    }
    println!("Funds deposited for users 1000-1100.");

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
        .set("socket.keepalive.enable", &config.kafka.socket_keepalive_enable)
        .create()
        .expect("Consumer creation failed");

    consumer.subscribe(&[&config.kafka.topic]).expect("Can't subscribe");

    println!(">>> Matching Engine Server Started");
    println!(">>> Listening on Topic: {}", config.kafka.topic);

    loop {
        match consumer.recv().await {
            Err(e) => eprintln!("Kafka error: {}", e),
            Ok(m) => {
                if let Some(payload) = m.payload_view::<str>() {
                    match payload {
                        Ok(text) => {
                            // Deserialize
                            if let Ok(req) = serde_json::from_str::<OrderRequest>(text) {
                                match req {
                                    OrderRequest::PlaceOrder { order_id, user_id, symbol_id, side, price, quantity, order_type } => {
                                        // Symbol is now u32 (ID). We can check if it exists in our manager or just pass it.
                                        // The engine will validate if the symbol ID is registered.
                                        // But we might want to log the string name.
                                        if let Some(symbol_name) = symbol_manager.get_symbol(symbol_id) {
                                            let side_enum = if side.eq_ignore_ascii_case("Buy") { Side::Buy } else { Side::Sell };
                                            let type_enum = if order_type.eq_ignore_ascii_case("Market") { OrderType::Market } else { OrderType::Limit };
                                            
                                            match engine.add_order(symbol_id, order_id, side_enum, type_enum, price, quantity, user_id) {
                                                Ok(_) => println!("Order {} Placed: {} {} @ {} ({})", order_id, side, quantity, price, symbol_name),
                                                Err(e) => eprintln!("Order {} Failed: {}", order_id, e),
                                            }
                                        } else {
                                            eprintln!("Unknown symbol ID: {}", symbol_id);
                                        }
                                    },
                                    OrderRequest::CancelOrder { order_id, symbol_id, .. } => {
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
                        },
                        Err(e) => eprintln!("Error reading payload: {}", e),
                    }
                }
            }
        }
    }
}
