use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message as KafkaMessage;
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

use fetcher::configure;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Centrifugo WebSocket URL
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = configure::load_config().expect("Failed to load config");

    let centrifugo_url = args.url.clone().unwrap_or(config.centrifugo_url.clone());
    let centrifugo_channel = args
        .channel
        .clone()
        .unwrap_or(config.centrifugo_channel.clone());
    let kafka_broker = args
        .kafka_broker
        .clone()
        .unwrap_or(config.kafka_broker.clone());
    let kafka_topic = args
        .kafka_topic
        .clone()
        .unwrap_or(config.kafka_topic.clone());
    let base_group_id = args
        .group_id
        .clone()
        .unwrap_or(config.kafka_group_id.clone());

    loop {
        println!("Starting bridge...");
        if let Err(e) = run_bridge(
            &centrifugo_url,
            &centrifugo_channel,
            &kafka_broker,
            &kafka_topic,
            &base_group_id,
        )
        .await
        {
            eprintln!("Bridge error: {}", e);
        } else {
            eprintln!("Bridge exited unexpectedly");
        }

        println!("Reconnecting in 1 seconds...");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

async fn run_bridge(
    centrifugo_url: &str,
    centrifugo_channel: &str,
    kafka_broker: &str,
    kafka_topic: &str,
    base_group_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let kafka_group_id = format!("{}-{}", base_group_id, chrono::Utc::now().timestamp());

    // 1. Generate JWT Token
    let now = chrono::Utc::now().timestamp() as usize;
    let my_claims = Claims {
        sub: "rust_client".to_owned(),
        exp: now + 10000000000, // Long expiration
    };
    let secret = "my_super_secret_key_which_is_very_long_and_secure_enough_for_hs256"; // TODO: Load from config
    let token = encode(
        &Header::default(),
        &my_claims,
        &EncodingKey::from_secret(secret.as_ref()),
    )?;
    println!("Generated Token: {}", token);

    // 2. Connect to Centrifugo
    let url = Url::parse(centrifugo_url)?;
    // url.query_pairs_mut().append_pair("token", &token); // Don't put token in URL

    println!("Connecting to Centrifugo at {}", url);
    let (ws_stream, _) = connect_async(url.to_string()).await?;
    println!("Connected to Centrifugo");

    let (mut write, mut read) = ws_stream.split();

    // Send Connect command
    let connect_msg = json!({
        "id": 1,
        "connect": {
            "token": token
        }
    });
    write
        .send(Message::Text(connect_msg.to_string().into()))
        .await?;
    println!("Sent connect message to Centrifugo");

    // Wait for connect reply (simple check)
    if let Some(msg) = read.next().await {
        let msg = msg?;
        println!("Received Centrifugo connect reply: {}", msg);
    }

    // 3. Connect to Redpanda
    println!("Connecting to Redpanda at {}", kafka_broker);
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", kafka_broker)
        .set("group.id", &kafka_group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .create()?;

    consumer.subscribe(&[kafka_topic])?;
    println!("Subscribed to topic: {}", kafka_topic);

    // 4. Consume and Forward Loop
    println!("Starting consume loop...");

    // We need to handle reading from WS (to keep connection alive/handle pings)
    // and reading from Kafka concurrently.
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(10));
    ping_interval.tick().await; // Consume first tick
    let mut command_id = 2; // Start from 2, as 1 was connect

    loop {
        tokio::select! {
            // Handle incoming WebSocket messages (pings, replies, etc.)
            ws_msg = read.next() => {
                match ws_msg {
                    Some(Ok(msg)) => {
                        match msg {
                            Message::Text(text) => {
                                // println!("Received Text: {}", text);
                                if text.as_str() == "{}" {
                                    // println!("Received empty JSON (Ping) from server, sending Pong...");
                                    if let Err(e) = write.send(Message::Text("{}".into())).await {
                                        eprintln!("Failed to send Pong: {}", e);
                                        return Err(Box::new(e));
                                    }
                                }
                            }
                            Message::Ping(data) => {
                                println!("Received Ping from server, sending Pong...");
                                if let Err(e) = write.send(Message::Pong(data)).await {
                                    eprintln!("Failed to send Pong: {}", e);
                                    return Err(Box::new(e));
                                }
                            }
                            Message::Close(frame) => {
                                println!("Centrifugo connection closed: {:?}", frame);
                                return Err("Centrifugo connection closed".into());
                            }
                            _ => {}
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("Centrifugo WebSocket error: {}", e);
                        return Err(Box::new(e));
                    }
                    None => {
                        println!("Centrifugo stream ended");
                        return Err("Centrifugo stream ended".into());
                    }
                }
            }

            // Handle incoming Kafka messages
            kafka_res = consumer.recv() => {
                match kafka_res {
                    Ok(m) => {
                        if let Some(payload) = m.payload() {
                            if let Ok(payload_str) = std::str::from_utf8(payload) {
                                println!("Received from Kafka: {}", payload_str);

                                // Construct Publish command using Command style
                                let publish_msg = json!({
                                    "id": command_id,
                                    "publish": {
                                        "channel": centrifugo_channel,
                                        "data": {
                                            "content": payload_str
                                        }
                                    }
                                });
                                command_id += 1;

                                let msg_str = publish_msg.to_string();
                                println!("Sending to Centrifugo: {}", msg_str);

                                if let Err(e) = write.send(Message::Text(msg_str.into())).await {
                                    eprintln!("Failed to send to Centrifugo: {}", e);
                                    return Err(Box::new(e));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Kafka receive error: {}", e);
                        // Don't exit on Kafka error, just log it? Or maybe exit to reconnect?
                        // For now, let's log and continue, unless it's fatal.
                    }
                }
            }
        }
    }
}
