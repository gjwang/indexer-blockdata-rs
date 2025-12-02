use clap::Parser;
use fetcher::configure;
use fetcher::models::{BalanceUpdate, OrderUpdate, PositionUpdate, StreamMessage, UserUpdate};
use rand::Rng;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Kafka/Redpanda Broker List
    #[arg(long)]
    kafka_broker: Option<String>,

    /// Kafka Topic to publish to
    #[arg(long)]
    kafka_topic: Option<String>,

    /// User ID to generate data for
    #[arg(long, default_value = "12345")]
    user_id: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = configure::load_config().expect("Failed to load config");

    let kafka_broker = args
        .kafka_broker
        .clone()
        .unwrap_or(config.kafka.broker.clone());
    let kafka_topic = args
        .kafka_topic
        .clone()
        .unwrap_or("user_updates".to_string()); // Default to user_updates topic
    let user_id = args.user_id.clone();

    println!("=== User Data Generator ===");
    println!("Broker: {}", kafka_broker);
    println!("Topic: {}", kafka_topic);
    println!("User ID: {}\n", user_id);

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &kafka_broker)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", &config.kafka.linger_ms)
        .create()?;

    println!("Producer created. Starting data generation loop...\n");

    let mut rng = rand::rng();

    loop {
        let update_type = rng.random_range(0..3);
        let ts_ms = chrono::Utc::now().timestamp_millis();

        let user_update = match update_type {
            0 => {
                // Balance Update
                let available = rng.random_range(0.5..2.0);
                let locked = rng.random_range(0.0..0.5);
                UserUpdate::Balance(BalanceUpdate {
                    asset: "BTC".into(),
                    available,
                    locked,
                    total: available + locked,
                })
            }
            1 => {
                // Order Update
                let price = rng.random_range(49000.0..51000.0);
                let quantity = rng.random_range(0.01..0.5);
                UserUpdate::Order(OrderUpdate {
                    order_id: format!("ord_{}", rng.random_range(1000..9999)),
                    symbol_id: 0, // BTC_USDT
                    side: if rng.random_bool(0.5) {
                        "buy".into()
                    } else {
                        "sell".into()
                    },
                    order_type: "limit".into(),
                    status: "new".into(),
                    price,
                    quantity,
                    filled_quantity: 0.0,
                    remaining_quantity: quantity,
                })
            }
            _ => {
                // Position Update
                let entry_price = rng.random_range(48000.0..50000.0);
                let mark_price = rng.random_range(49000.0..51000.0);
                UserUpdate::Position(PositionUpdate {
                    symbol_id: 0, // BTC_USDT
                    side: "long".into(),
                    quantity: rng.random_range(0.1..1.0),
                    entry_price,
                    mark_price,
                    liquidation_price: entry_price * 0.8,
                    unrealized_pnl: (mark_price - entry_price) * 0.1, // Simplified PnL
                    leverage: 10.0,
                })
            }
        };

        let stream_message = StreamMessage {
            ts_ms,
            update: user_update.clone(),
        };

        let payload = serde_json::to_string(&stream_message)?;
        let key = user_id.clone();

        match &user_update {
            UserUpdate::Balance(_) => println!("Sending Balance update for user {}", user_id),
            UserUpdate::Order(_) => println!("Sending Order update for user {}", user_id),
            UserUpdate::Position(_) => println!("Sending Position update for user {}", user_id),
        }

        let record = FutureRecord::to(&kafka_topic).key(&key).payload(&payload);

        match producer.send(record, Duration::from_secs(5)).await {
            Ok(_) => println!("✓ Sent to Kafka"),
            Err((e, _)) => eprintln!("✗ Failed to send: {:?}", e),
        }

        // Wait for a random interval between 1 and 3 seconds
        let sleep_millis = rng.random_range(1000..3000);
        tokio::time::sleep(Duration::from_millis(sleep_millis)).await;
    }
}
