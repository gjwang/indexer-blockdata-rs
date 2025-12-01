use anyhow::Result;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::Message;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug)]
struct DemoMessage {
    id: u32,
    content: String,
    timestamp: i64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Redpanda 连接配置
    let broker = "localhost:9092";
    let topic = "demo-topic";
    let group_id = "demo-consumer-group";

    println!("=== Redpanda 读写示例 ===\n");

    // 创建生产者
    let producer = create_producer(broker)?;

    // 发送消息
    println!("📤 发送消息到 Redpanda...");
    send_messages(&producer, topic).await?;

    // 等待一下让消息被处理
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 创建消费者
    let consumer = create_consumer(broker, group_id)?;
    consumer.subscribe(&[topic])?;

    // 接收消息
    println!("\n📥 从 Redpanda 接收消息...");
    receive_messages(&consumer).await?;

    Ok(())
}

/// 创建 Kafka/Redpanda 生产者
fn create_producer(broker: &str) -> Result<FutureProducer> {
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "10000")
        .create()?;

    println!("✅ 生产者创建成功");
    Ok(producer)
}

/// 创建 Kafka/Redpanda 消费者
fn create_consumer(broker: &str, group_id: &str) -> Result<StreamConsumer> {
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("group.id", group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "earliest")
        .set("session.timeout.ms", "6000")
        .create()?;

    println!("✅ 消费者创建成功");
    Ok(consumer)
}

/// 发送多条消息到 Redpanda
async fn send_messages(producer: &FutureProducer, topic: &str) -> Result<()> {
    for i in 1..=5 {
        let msg = DemoMessage {
            id: i,
            content: format!("这是第 {} 条消息", i),
            timestamp: chrono::Utc::now().timestamp(),
        };

        let payload = serde_json::to_string(&msg)?;
        let key = format!("key-{}", i);

        let record = FutureRecord::to(topic).key(&key).payload(&payload);

        match producer.send(record, Duration::from_secs(0)).await {
            Ok(delivery) => {
                println!(
                    "  ✓ 消息 {} 发送成功: partition={}, offset={}",
                    i, delivery.0, delivery.1
                );
            }
            Err((e, _)) => {
                eprintln!("  ✗ 消息 {} 发送失败: {:?}", i, e);
            }
        }
    }

    Ok(())
}

/// 从 Redpanda 接收消息
async fn receive_messages(consumer: &StreamConsumer) -> Result<()> {
    let mut count = 0;
    let max_messages = 5;

    loop {
        match consumer.recv().await {
            Ok(msg) => {
                if let Some(payload) = msg.payload() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(demo_msg) = serde_json::from_str::<DemoMessage>(text) {
                            println!("  ✓ 收到消息: {:?}", demo_msg);
                            println!(
                                "    - Partition: {}, Offset: {}",
                                msg.partition(),
                                msg.offset()
                            );

                            count += 1;
                            if count >= max_messages {
                                break;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("  ✗ 接收消息错误: {:?}", e);
                break;
            }
        }
    }

    println!("\n📊 共接收 {} 条消息", count);
    Ok(())
}
