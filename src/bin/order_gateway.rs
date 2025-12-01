use fetcher::fast_ulid::SnowflakeGenRng;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;
use tokio::time;

use fetcher::models::{OrderRequest, OrderType, Side};
use fetcher::symbol_manager::SymbolManager;

use fetcher::models::ClientRawOrder;

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
    let count: u64 = 1000000;
    let interval_ms = 100;

    for i in 0..count {
        // 1. Simulate receiving raw order data
        let raw_symbol = symbols[i as usize % symbols.len()];
        let raw_side = if i % 2 == 0 { "Buy" } else { "Sell" };
        let raw_type = "Limit";
        let price = 50000 + (i % 100); // Realistic BTC price
        let quantity = 1 + (i % 5);
        let user_id = 1000 + (i % 10);
        let order_id = snowflake_gen.generate();

        let client_order = ClientRawOrder {
            symbol: raw_symbol.to_string(),
            side: raw_side.to_string(),
            price,
            quantity,
            user_id,
            order_type: raw_type.to_string(),
        };

        let order = match client_order.to_internal(&symbol_manager, order_id) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("Error converting order: {}", e);
                continue;
            }
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
