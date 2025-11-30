use clap::Parser;
use fetcher::centrifugo_publisher::CentrifugoPublisher;
use fetcher::configure;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Centrifugo HTTP API URL
    #[arg(long)]
    centrifugo_url: Option<String>,

    /// Centrifugo API Key
    #[arg(long)]
    api_key: Option<String>,

    /// Kafka/Redpanda Broker List
    #[arg(long)]
    kafka_broker: Option<String>,

    /// Kafka Topic to consume from
    #[arg(long)]
    kafka_topic: Option<String>,

    /// Kafka Group ID
    #[arg(long)]
    group_id: Option<String>,

    /// Use a random group ID (useful for development to avoid rebalance delays)
    #[arg(long)]
    random_group_id: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = configure::load_config().expect("Failed to load config");

    let centrifugo_url = args.centrifugo_url.clone()
        .unwrap_or_else(|| "http://localhost:8000/api".to_string());
    let api_key = args.api_key
        .unwrap_or_else(|| std::env::var("CENTRIFUGO_API_KEY")
            .unwrap_or_else(|_| "your_secure_api_key_here_change_this_in_production".to_string()));
    let kafka_broker = args.kafka_broker.clone()
        .unwrap_or(config.kafka_broker.clone());
    let kafka_topic = args.kafka_topic.clone()
        .unwrap_or("user_updates".to_string());
    
    let group_id = if args.random_group_id {
        format!("centrifugo-bridge-{}", uuid::Uuid::new_v4())
    } else {
        args.group_id.clone().unwrap_or("centrifugo-bridge".to_string())
    };

    println!("=== Centrifugo Bridge ===");
    println!("Centrifugo URL: {}", centrifugo_url);
    println!("Kafka Broker: {}", kafka_broker);
    println!("Topic: {}", kafka_topic);
    println!("Group ID: {}\n", group_id);

    // Initialize Publisher
    let publisher = CentrifugoPublisher::new(centrifugo_url, api_key);

    // Initialize Consumer
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &kafka_broker)
        .set("group.id", &group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .set("session.timeout.ms", &config.kafka_session_timeout_ms)
        .set("heartbeat.interval.ms", &config.kafka_heartbeat_interval_ms)
        .set("max.poll.interval.ms", &config.kafka_max_poll_interval_ms)
        .set("socket.keepalive.enable", &config.kafka_socket_keepalive_enable)
        .set("fetch.wait.max.ms", &config.kafka_fetch_wait_max_ms)
        .create()?;

    consumer.subscribe(&[&kafka_topic])?;
    println!("Subscribed to topic. Waiting for messages...\n");

    loop {
        match consumer.recv().await {
            Ok(m) => {
                if let Some(payload) = m.payload() {
                    if let Ok(payload_str) = std::str::from_utf8(payload) {
                        println!("Received update: {}", payload_str);

                        // We assume the key is the user_id
                        let user_id = match m.key() {
                            Some(k) => std::str::from_utf8(k).unwrap_or("unknown"),
                            None => "unknown",
                        };

                        if user_id == "unknown" {
                            eprintln!("Skipping message with no user_id key");
                            continue;
                        }

                        // Publish raw JSON directly without deserializing
                        let result = publisher.publish_raw_json(user_id, payload_str).await;

                        match result {
                            Ok(_) => println!("✓ Pushed to Centrifugo for user: {}", user_id),
                            Err(e) => eprintln!("✗ Centrifugo push failed: {}", e),
                        }
                    }
                }
            }
            Err(e) => eprintln!("Kafka error: {}", e),
        }
    }
}
