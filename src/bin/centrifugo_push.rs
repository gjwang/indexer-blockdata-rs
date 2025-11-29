use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;
use serde_json::json;
use clap::Parser;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message as KafkaMessage;

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

    let centrifugo_url = args.url.unwrap_or(config.centrifugo_url);
    let centrifugo_channel = args.channel.unwrap_or(config.centrifugo_channel);
    let kafka_broker = args.kafka_broker.unwrap_or(config.kafka_broker);
    let kafka_topic = args.kafka_topic.unwrap_or(config.kafka_topic);
    let kafka_group_id = args.group_id.unwrap_or(config.kafka_group_id);

    // 1. Connect to Centrifugo
    let url = Url::parse(&centrifugo_url)?;
    println!("Connecting to Centrifugo at {}", url);
    let (ws_stream, _) = connect_async(url).await.expect("Failed to connect to Centrifugo");
    println!("Connected to Centrifugo");

    let (mut write, mut read) = ws_stream.split();

    // Send Connect command
    let connect_msg = json!({
        "id": 1,
        "connect": {}
    });
    write.send(Message::Text(connect_msg.to_string())).await?;
    println!("Sent connect message to Centrifugo");

    // Wait for connect reply (simple check)
    if let Some(msg) = read.next().await {
        let msg = msg?;
        println!("Received Centrifugo connect reply: {}", msg);
    }

    // 2. Connect to Redpanda
    println!("Connecting to Redpanda at {}", kafka_broker);
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &kafka_broker)
        .set("group.id", &kafka_group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .create()?;

    consumer.subscribe(&[&kafka_topic])?;
    println!("Subscribed to topic: {}", kafka_topic);

    // 3. Consume and Forward Loop
    println!("Starting consume loop...");
    
    // We need to handle reading from WS (to keep connection alive/handle pings) 
    // and reading from Kafka concurrently.
    
    loop {
        tokio::select! {
            // Handle incoming WebSocket messages (pings, replies, etc.)
            ws_msg = read.next() => {
                match ws_msg {
                    Some(Ok(msg)) => {
                        if msg.is_close() {
                            println!("Centrifugo connection closed");
                            break;
                        }
                        // Ignore other messages for now, or log them
                        // println!("Received from Centrifugo: {}", msg);
                    }
                    Some(Err(e)) => {
                        eprintln!("Centrifugo WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        println!("Centrifugo stream ended");
                        break;
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

                                // Construct Publish command
                                let publish_msg = json!({
                                    "publish": {
                                        "channel": centrifugo_channel,
                                        "data": {
                                            "content": payload_str
                                        }
                                    }
                                });

                                if let Err(e) = write.send(Message::Text(publish_msg.to_string())).await {
                                    eprintln!("Failed to send to Centrifugo: {}", e);
                                    break;
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
    }

    Ok(())
}
