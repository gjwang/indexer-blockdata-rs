use clap::Parser;
use fetcher::configure;
use fetcher::models::BalanceUpdate;
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

    let kafka_broker = args.kafka_broker.clone()
        .unwrap_or(config.kafka_broker.clone());
    let kafka_topic = args.kafka_topic.clone()
        .unwrap_or("user_updates".to_string()); // Default to user_updates topic
    let user_id = args.user_id;

    println!("=== User Data Generator ===");
    println!("Broker: {}", kafka_broker);
    println!("Topic: {}", kafka_topic);
    println!("User ID: {}\n", user_id);

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &kafka_broker)
        .set("message.timeout.ms", "5000")
        .create()?;

    println!("Producer created. Starting data generation loop...\n");

    let mut rng = rand::thread_rng();

    loop {
        // Generate random balance update
        let update = BalanceUpdate {
            asset: "BTC".to_string(),
            available: rng.gen_range(0.5..2.0),
            locked: rng.gen_range(0.0..0.5),
            total: 0.0, // Calculated below
            timestamp: chrono::Utc::now().timestamp(),
        };
        let mut update = update;
        update.total = update.available + update.locked;

        let payload = serde_json::to_string(&update)?;
        let key = user_id.clone();

        println!("Sending update for user {}: {} BTC", user_id, update.total);

        let record = FutureRecord::to(&kafka_topic)
            .key(&key)
            .payload(&payload);

        match producer.send(record, Duration::from_secs(5)).await {
            Ok(_) => println!("✓ Sent to Kafka"),
            Err((e, _)) => eprintln!("✗ Failed to send: {:?}", e),
        }

        // Wait for a random interval between 1 and 3 seconds
        let sleep_millis = rng.gen_range(1000..3000);
        tokio::time::sleep(Duration::from_millis(sleep_millis)).await;
    }
}
