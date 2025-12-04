use anyhow::Result;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Trade {
    trade_id: u64,
    #[serde(default)]
    match_seq: u64,
    buy_order_id: u64,
    sell_order_id: u64,
    buy_user_id: u64,
    sell_user_id: u64,
    price: u64,
    quantity: u64,
}

#[derive(Debug, Serialize)]
struct StarRocksTrade {
    trade_id: i64,
    match_seq: i64,
    buy_order_id: i64,
    sell_order_id: i64,
    buy_user_id: i64,
    sell_user_id: i64,
    price: i64,
    quantity: i64,
    trade_date: String,
    settled_at: String,
}

impl StarRocksTrade {
    fn from_trade(trade: Trade, timestamp_ms: i64) -> Self {
        let datetime = chrono::DateTime::from_timestamp_millis(timestamp_ms)
            .unwrap_or_else(|| chrono::Utc::now());

        Self {
            trade_id: trade.trade_id as i64,
            match_seq: trade.match_seq as i64,
            buy_order_id: trade.buy_order_id as i64,
            sell_order_id: trade.sell_order_id as i64,
            buy_user_id: trade.buy_user_id as i64,
            sell_user_id: trade.sell_user_id as i64,
            price: trade.price as i64,
            quantity: trade.quantity as i64,
            trade_date: datetime.format("%Y-%m-%d").to_string(),
            settled_at: datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== StarRocks Bridge Service ===");
    println!("Consuming from: trade.history");
    println!("Loading to: StarRocks settlement.trades");
    println!("--------------------------------");

    // Kafka consumer
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", "localhost:9093")
        .set("group.id", "starrocks_bridge")
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "earliest")
        .create()?;

    consumer.subscribe(&["trade.history"])?;

    // HTTP client for StarRocks Stream Load
    let http_client = Client::new();
    let starrocks_url = "http://localhost:8040/api/settlement/trades/_stream_load";

    let mut batch = Vec::new();
    const BATCH_SIZE: usize = 100;
    let mut label_counter = 0u64;

    println!("✅ Started consuming trades...\n");

    loop {
        match consumer.recv().await {
            Ok(msg) => {
                if let Some(payload) = msg.payload_view::<str>() {
                    match payload {
                        Ok(text) => {
                            // Parse trade array
                            match serde_json::from_str::<Vec<Trade>>(text) {
                                Ok(trades) => {
                                    let timestamp_ms = msg.timestamp().to_millis().unwrap_or(0);

                                    for trade in trades {
                                        let sr_trade = StarRocksTrade::from_trade(trade, timestamp_ms);
                                        batch.push(sr_trade);
                                    }

                                    // Flush batch when full
                                    if batch.len() >= BATCH_SIZE {
                                        if let Err(e) = flush_batch(&http_client, starrocks_url, &mut batch, &mut label_counter).await {
                                            eprintln!("❌ Failed to flush batch: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to parse trades: {} | Payload: {}", e, text);
                                }
                            }
                        }
                        Err(e) => eprintln!("Error reading payload: {}", e),
                    }
                }
            }
            Err(e) => {
                eprintln!("Kafka error: {}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn flush_batch(
    client: &Client,
    url: &str,
    batch: &mut Vec<StarRocksTrade>,
    label_counter: &mut u64,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    *label_counter += 1;
    let label = format!("bridge_batch_{}", label_counter);

    // Convert batch to newline-delimited JSON
    let mut payload = String::new();
    for trade in batch.iter() {
        payload.push_str(&serde_json::to_string(trade)?);
        payload.push('\n');
    }

    let response = client
        .put(url)
        .basic_auth("root", Some(""))
        .header("label", &label)
        .header("format", "json")
        .body(payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if status.is_success() {
        let result: serde_json::Value = serde_json::from_str(&body)?;
        if result["Status"] == "Success" {
            println!("✅ Loaded {} trades (label: {})", batch.len(), label);
            batch.clear();
        } else {
            eprintln!("❌ Load failed: {}", result["Message"]);
            eprintln!("   Response: {}", body);
        }
    } else {
        eprintln!("❌ HTTP error {}: {}", status, body);
    }

    Ok(())
}
