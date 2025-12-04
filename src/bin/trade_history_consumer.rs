use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::Message;

use fetcher::configure;
use fetcher::models::Trade;

#[tokio::main]
async fn main() {
    let config = configure::load_config().expect("Failed to load config");

    let broker = config.kafka.broker;
    let topic = config.kafka.topics.trades;
    let group_id = format!("{}-history-viewer", config.kafka.group_id);

    println!("=== Trade History Consumer ===");
    println!("Broker: {}", broker);
    println!("Topic: {}", topic);
    println!("Group ID: {}", group_id);
    println!("------------------------------");

    // Initialize Consumer
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &broker)
        .set("group.id", &group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .set("session.timeout.ms", &config.kafka.session_timeout_ms)
        .set("heartbeat.interval.ms", &config.kafka.heartbeat_interval_ms)
        .set("max.poll.interval.ms", &config.kafka.max_poll_interval_ms)
        .set("socket.keepalive.enable", &config.kafka.socket_keepalive_enable)
        .set("fetch.wait.max.ms", &config.kafka.fetch_wait_max_ms)
        .create()
        .unwrap();

    consumer.subscribe(&[&topic]).expect("Can't subscribe");
    println!("Listening for trades...");

    loop {
        match consumer.recv().await {
            Err(e) => match e {
                KafkaError::MessageConsumption(RDKafkaErrorCode::UnknownTopicOrPartition) => {
                    eprintln!("Topic '{}' not found yet. Waiting for trades...", topic);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                _ => eprintln!("Kafka error: {}", e),
            },
            Ok(m) => {
                if let Some(payload) = m.payload_view::<str>() {
                    match payload {
                        Ok(text) => {
                            // Deserialize JSON array of trades
                            match serde_json::from_str::<Vec<Trade>>(text) {
                                Ok(trades) => {
                                    for trade in trades {
                                        println!(
                                            "Trade #{}: Price={} Qty={} (BuyOrder={}, SellOrder={} | Buyer={}, Seller={})",
                                            trade.trade_id,
                                            trade.price,
                                            trade.quantity,
                                            trade.buy_order_id,
                                            trade.sell_order_id,
                                            trade.buy_user_id,
                                            trade.sell_user_id
                                        );
                                    }
                                }
                                Err(e) => eprintln!(
                                    "Failed to parse trade batch: {} | Payload: {}",
                                    e, text
                                ),
                            }
                        }
                        Err(e) => eprintln!("Error reading payload: {}", e),
                    }
                }
            }
        }
    }
}
