use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::models::OrderRequest;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;
use tokio::time;

use fetcher::symbol_manager::SymbolManager;

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    // Initialize SymbolManager to map strings to IDs
    let symbol_manager = SymbolManager::load_from_db();

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", &config.kafka.linger_ms)
        .set(
            "socket.keepalive.enable",
            &config.kafka.socket_keepalive_enable,
        )
        .create()
        .expect("Producer creation error");

    let mut snowflake_gen = SnowflakeGenRng::new(1);
    // Simulate raw input symbols
    let symbols: Vec<&str> = vec!["BTC_USDT", "ETH_USDT"];

    println!(">>> Starting Order Gateway (Producer)");
    println!(
        ">>> Target: {}, Topic: {}",
        config.kafka.broker, config.kafka.topic
    );

    // Default count and interval if not in config (could add to AppConfig if needed)
    let count = 1000000;
    let interval_ms = 100;

    for i in 0..count {
        // 1. Simulate receiving raw order data
        let raw_symbol = symbols[i % symbols.len()];
        let side = if i % 2 == 0 { "Buy" } else { "Sell" }.to_string();
        let price = 50000 + (i as u64 % 100); // Realistic BTC price
        let quantity = 1 + (i as u64 % 5);
        let user_id = 1000 + (i as u64 % 10);
        let order_id = snowflake_gen.generate();

        // 2. Map symbol string to ID
        let symbol_id = match symbol_manager.get_id(raw_symbol) {
            Some(id) => id,
            None => {
                eprintln!("Error: Unknown symbol {}", raw_symbol);
                continue;
            }
        };

        let order = OrderRequest::PlaceOrder {
            order_id,
            user_id,
            symbol_id,
            side,
            price,
            quantity,
            order_type: "Limit".to_string(),
        };

        let payload = serde_json::to_string(&order).unwrap();
        let key = order_id.to_string();

        let record = FutureRecord::to(&config.kafka.topic)
            .payload(&payload)
            .key(&key);

        match producer.send(record, Duration::from_secs(0)).await {
            Ok((partition, offset)) => println!("Sent Order {} by user_id: {}", order_id, user_id),
            Err((e, _)) => eprintln!("Error sending order {}: {:?}", order_id, e),
        }

        time::sleep(Duration::from_millis(interval_ms)).await;
    }
}
