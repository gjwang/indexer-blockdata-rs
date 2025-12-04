# Centrifugo HTTP API Publishing Guide

## Recommended Production Setup

For a secure production setup where:
- Your `centrifugo_push` app is on a **private network**
- Public clients only **subscribe** (not publish)

You should use **Centrifugo's HTTP API** for server-side publishing.

## Configuration Changes

### 1. Update `config.json`

```json
{
    "client": {
        "token": {
            "hmac_secret_key": "my_super_secret_key_which_is_very_long_and_secure_enough_for_hs256"
        },
        "allowed_origins": ["*"]
    },
    "channel": {
        "namespaces": [
            {
                "name": "news",
                "allow_subscribe_for_client": true,
                "history_size": 100,
                "history_ttl": "300s",
                "join_leave": true,
                "presence": true
            }
        ]
    },
    "api_key": "your_secure_api_key_here_change_this_in_production",
    "admin": {
        "enabled": true,
        "password": "admin_password",
        "secret": "admin_secret"
    }
}
```

**Key changes:**
- ❌ Removed `allow_publish_for_client` - Clients can't publish
- ✅ Kept `allow_subscribe_for_client` - Clients can subscribe
- ✅ Added `api_key` - For HTTP API authentication

### 2. Add HTTP Client Dependency

Add to `Cargo.toml`:
```toml
reqwest = { version = "0.12", features = ["json"] }
```

### 3. Create HTTP-based Publisher

Create a new file `src/bin/centrifugo_push_http.rs`:

```rust
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
            .expect("CENTRIFUGO_API_KEY must be set"));

    loop {
        println!("Starting HTTP bridge...");
        if let Err(e) = run_http_bridge(
            &centrifugo_url,
            &centrifugo_channel,
            &kafka_broker,
            &kafka_topic,
            &base_group_id,
            &api_key,
        ).await {
            eprintln!("Bridge error: {}", e);
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

    // Connect to Redpanda
    println!("Connecting to Redpanda at {}", kafka_broker);
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", kafka_broker)
        .set("group.id", &kafka_group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .create()?;

    consumer.subscribe(&[kafka_topic])?;
    println!("Subscribed to topic: {}", kafka_topic);
    println!("Publishing to Centrifugo channel: {}", centrifugo_channel);

    // Consume and publish loop
    loop {
        match consumer.recv().await {
            Ok(m) => {
                if let Some(payload) = m.payload() {
                    if let Ok(payload_str) = std::str::from_utf8(payload) {
                        println!("Received from Kafka: {}", payload_str);

                        // Publish via HTTP API
                        let body = json!({
                            "channel": centrifugo_channel,
                            "data": {
                                "content": payload_str
                            }
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
                                    println!("✓ Published to Centrifugo");
                                } else {
                                    eprintln!("✗ Centrifugo error: {} - {}", 
                                        response.status(), 
                                        response.text().await.unwrap_or_default());
                                }
                            }
                            Err(e) => {
                                eprintln!("✗ HTTP request failed: {}", e);
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
```

### 4. Update `Cargo.toml`

Add the binary target:
```toml
[[bin]]
name = "centrifugo_push_http"
path = "src/bin/centrifugo_push_http.rs"
```

## Benefits of HTTP API Approach

✅ **More Secure**: No client-side publish permissions needed
✅ **Simpler**: No WebSocket connection management
✅ **More Reliable**: HTTP retries are easier to implement
✅ **Production Ready**: Recommended by Centrifugo for server-to-server communication
✅ **Better Separation**: Clear distinction between server (publish) and clients (subscribe)

## Running the HTTP Version

```bash
# Set API key as environment variable
export CENTRIFUGO_API_KEY="your_secure_api_key_here"

# Run the HTTP-based publisher
cargo run --bin centrifugo_push_http
```

## Current WebSocket Approach (Keep for Reference)

Your current setup uses client WebSocket publishing, which works but requires:
- `allow_publish_for_client: true` in config
- More complex connection management
- Less secure for production (any client with a valid JWT can publish)

The HTTP API approach is recommended for production deployments.
