use clap::Parser;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message as KafkaMessage;
use serde_json::json;
use fetcher::configure;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Centrifugo HTTP API URL
    #[arg(long)]
    url: Option<String>,

    /// Channel to publish to
    #[arg(long)]
    channel: Option<String>,

    /// Kafka/Redpanda Broker List
    #[arg(long)]
    kafka_broker: Option<String>,

    /// Kafka Topic to consume from
    #[arg(long)]
    kafka_topic: Option<String>,

    /// Kafka Group ID
    #[arg(long)]
    group_id: Option<String>,

    /// Centrifugo API Key
    #[arg(long)]
    api_key: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = configure::load_config().expect("Failed to load config");

    let centrifugo_url = args.url.clone()
        .unwrap_or_else(|| "http://localhost:8000/api".to_string());
    let centrifugo_channel = args.channel.clone()
        .unwrap_or(config.centrifugo_channel.clone());
    let kafka_broker = args.kafka_broker.clone()
        .unwrap_or(config.kafka_broker.clone());
    let kafka_topic = args.kafka_topic.clone()
        .unwrap_or(config.kafka_topic.clone());
    let base_group_id = args.group_id.clone()
        .unwrap_or(config.kafka_group_id.clone());
    let api_key = args.api_key
        .unwrap_or_else(|| std::env::var("CENTRIFUGO_API_KEY")
            .unwrap_or_else(|_| "your_secure_api_key_here_change_this_in_production".to_string()));

    loop {
        println!("Starting HTTP API bridge...");
        if let Err(e) = run_http_bridge(
            &centrifugo_url,
            &centrifugo_channel,
            &kafka_broker,
            &kafka_topic,
            &base_group_id,
            &api_key,
        ).await {
            eprintln!("Bridge error: {}", e);
        } else {
            eprintln!("Bridge exited unexpectedly");
        }

        println!("Reconnecting in 5 seconds...");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn run_http_bridge(
    centrifugo_url: &str,
    centrifugo_channel: &str,
    kafka_broker: &str,
    kafka_topic: &str,
    base_group_id: &str,
    api_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let kafka_group_id = format!("{}-{}", base_group_id, chrono::Utc::now().timestamp());

    // Create HTTP client
    let client = reqwest::Client::new();
    let publish_url = format!("{}/publish", centrifugo_url);

    println!("Centrifugo HTTP API URL: {}", publish_url);
    println!("Using API Key: {}...", &api_key.chars().take(10).collect::<String>());

    // Connect to Redpanda
    println!("Connecting to Redpanda at {}", kafka_broker);
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", kafka_broker)
        .set("group.id", &kafka_group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .create()?;

    consumer.subscribe(&[kafka_topic])?;
    println!("Subscribed to Kafka topic: {}", kafka_topic);
    println!("Publishing to Centrifugo channel: {}", centrifugo_channel);
    println!("Bridge is running...\n");

    // Consume and publish loop
    loop {
        match consumer.recv().await {
            Ok(m) => {
                if let Some(payload) = m.payload() {
                    if let Ok(payload_str) = std::str::from_utf8(payload) {
                        println!("Received from Kafka: {}", payload_str);

                        // Parse JSON payload
                        let data = match serde_json::from_str::<serde_json::Value>(payload_str) {
                            Ok(json_data) => json_data,
                            Err(e) => {
                                eprintln!("✗ Failed to parse JSON payload: {}\n", e);
                                continue;
                            }
                        };

                        // Publish via HTTP API
                        let body = json!({
                            "channel": centrifugo_channel,
                            "data": data
                        });

                        match client
                            .post(&publish_url)
                            .header("X-API-Key", api_key)
                            .header("Content-Type", "application/json")
                            .json(&body)
                            .send()
                            .await
                        {
                            Ok(response) => {
                                if response.status().is_success() {
                                    println!("✓ Published to Centrifugo\n");
                                } else {
                                    let status = response.status();
                                    let error_text = response.text().await.unwrap_or_default();
                                    eprintln!("✗ Centrifugo HTTP error: {} - {}\n", status, error_text);
                                }
                            }
                            Err(e) => {
                                eprintln!("✗ HTTP request failed: {}\n", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Kafka receive error: {}", e);
            }
        }
    }
}
