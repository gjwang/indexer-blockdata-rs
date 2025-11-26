// Cargo.toml 依赖:
// [dependencies]
// rdkafka = { version = "0.36", features = ["cmake-build"] }
// tokio = { version = "1", features = ["full"] }
// serde = { version = "1.0", features = ["derive"] }
// serde_json = "1.0"
// anyhow = "1.0"
// chrono = "0.4"

use anyhow::Result;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::Message;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct DemoMessage {
    id: u32,
    content: String,
}

#[derive(Debug, Clone)]
struct LatencyRecord {
    msg_id: u32,
    send_time: Instant,
    receive_time: Option<Instant>,
    producer_latency: Duration,
}

#[derive(Debug)]
struct LatencyStats {
    min: Duration,
    max: Duration,
    avg: Duration,
    p50: Duration,
    p95: Duration,
    p99: Duration,
}

#[tokio::main]
async fn main() -> Result<()> {
    let broker = "localhost:9092";
    let topic = "latency-test-topic";
    let group_id = "latency-consumer-group";

    println!("=== Redpanda 延迟测试 ===\n");

    // 创建生产者和消费者
    let producer = create_producer(broker)?;
    let consumer = create_consumer(broker, group_id)?;
    consumer.subscribe(&[topic])?;

    // 运行延迟测试
    println!("🚀 开始延迟测试...\n");
    run_latency_test(&producer, &consumer, topic, 100).await?;

    Ok(())
}

fn create_producer(broker: &str) -> Result<FutureProducer> {
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", "0") // 立即发送，减少批处理延迟
        .set("compression.type", "none") // 禁用压缩以减少延迟
        .set("acks", "all") // 等待所有副本确认
        .create()?;

    println!("✅ 生产者创建成功");
    Ok(producer)
}

fn create_consumer(broker: &str, group_id: &str) -> Result<StreamConsumer> {
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("group.id", group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest") // 只读取新消息
        .set("fetch.min.bytes", "1") // 立即获取消息
        .create()?;

    println!("✅ 消费者创建成功");
    Ok(consumer)
}

async fn run_latency_test(
    producer: &FutureProducer,
    consumer: &StreamConsumer,
    topic: &str,
    message_count: usize,
) -> Result<()> {
    // 使用 Arc<Mutex> 来共享延迟记录
    let latency_records = Arc::new(Mutex::new(Vec::<LatencyRecord>::new()));
    let records_for_consumer = latency_records.clone();
    let records_for_send = latency_records.clone();

    // 启动消费者任务（不使用 spawn，直接在当前任务中并发）
    println!("📡 启动消费者监听...");

    // 等待消费者准备好
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("📤 开始发送 {} 条测试消息...\n", message_count);

    // 同时启动发送和接收任务
    let send_task = async { send_messages(producer, topic, records_for_send, message_count).await };

    let receive_task =
        async { consume_messages(consumer, records_for_consumer, message_count).await };

    // 并发执行发送和接收
    tokio::join!(send_task, receive_task);

    println!("\n✅ 测试完成，计算统计信息...\n");

    // 计算并显示统计信息
    let records = latency_records.lock().await;
    print_latency_stats(&records);

    Ok(())
}

async fn send_messages(
    producer: &FutureProducer,
    topic: &str,
    records: Arc<Mutex<Vec<LatencyRecord>>>,
    message_count: usize,
) {
    // 发送消息并记录发送时间
    for i in 0..message_count {
        let msg = DemoMessage {
            id: i as u32,
            content: format!("延迟测试消息 {}", i),
        };

        let payload = serde_json::to_string(&msg).unwrap();
        let key = format!("key-{}", i);

        let send_start = Instant::now();
        let record_msg = FutureRecord::to(topic).key(&key).payload(&payload);

        match producer.send(record_msg, Duration::from_secs(5)).await {
            Ok(_) => {
                let producer_latency = send_start.elapsed();

                // 记录发送信息
                let mut recs = records.lock().await;
                recs.push(LatencyRecord {
                    msg_id: i as u32,
                    send_time: send_start,
                    receive_time: None,
                    producer_latency,
                });

                if (i + 1) % 20 == 0 {
                    println!("  已发送 {}/{} 条消息", i + 1, message_count);
                }
            }
            Err((e, _)) => {
                eprintln!("  ✗ 消息 {} 发送失败: {:?}", i, e);
            }
        }

        // 控制发送速率，避免过快
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    println!("\n✅ 消息发送完成");
}

async fn consume_messages(
    consumer: &StreamConsumer,
    records: Arc<Mutex<Vec<LatencyRecord>>>,
    expected_count: usize,
) {
    let mut received_count = 0;
    println!("📥 消费者开始接收消息...\n");

    loop {
        match tokio::time::timeout(Duration::from_secs(5), consumer.recv()).await {
            Ok(Ok(msg)) => {
                let receive_time = Instant::now();

                if let Some(payload) = msg.payload() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(demo_msg) = serde_json::from_str::<DemoMessage>(text) {
                            // 更新对应消息的接收时间
                            let mut recs = records.lock().await;
                            if let Some(record) = recs.iter_mut().find(|r| r.msg_id == demo_msg.id)
                            {
                                record.receive_time = Some(receive_time);
                                received_count += 1;

                                if received_count % 20 == 0 {
                                    println!(
                                        "  已接收 {}/{} 条消息",
                                        received_count, expected_count
                                    );
                                }

                                if received_count >= expected_count {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("  接收错误: {:?}", e);
            }
            Err(_) => {
                println!("  接收超时，已接收 {} 条消息", received_count);
                break;
            }
        }
    }
}

fn print_latency_stats(records: &[LatencyRecord]) {
    let mut producer_latencies = Vec::new();
    let mut e2e_latencies = Vec::new();
    let mut received_count = 0;

    for record in records {
        producer_latencies.push(record.producer_latency);

        if let Some(receive_time) = record.receive_time {
            let e2e_latency = receive_time.duration_since(record.send_time);
            e2e_latencies.push(e2e_latency);
            received_count += 1;
        }
    }

    println!("\n{}", "=".repeat(60));
    println!("📊 延迟测试结果");
    println!("{}", "=".repeat(60));

    println!("\n📈 测试统计:");
    println!("  总发送消息数: {}", records.len());
    println!("  成功接收数:   {}", received_count);
    println!(
        "  丢失率:        {:.2}%",
        (records.len() - received_count) as f64 / records.len() as f64 * 100.0
    );

    if !producer_latencies.is_empty() {
        let producer_stats = calculate_stats(&producer_latencies);
        print_stats("🚀 生产者发送延迟", &producer_stats);
    }

    if !e2e_latencies.is_empty() {
        let e2e_stats = calculate_stats(&e2e_latencies);
        print_stats("🔄 端到端延迟 (发送→接收)", &e2e_stats);

        // 计算消费者延迟（近似）
        if !producer_latencies.is_empty() {
            let producer_stats = calculate_stats(&producer_latencies);
            let avg_network_consumer = e2e_stats.avg.saturating_sub(producer_stats.avg);
            println!("\n📍 网络+消费者延迟（近似）: {:?}", avg_network_consumer);
        }
    }

    println!("\n{}", "=".repeat(60));
}

fn calculate_stats(latencies: &[Duration]) -> LatencyStats {
    let mut sorted = latencies.to_vec();
    sorted.sort();

    let sum: Duration = sorted.iter().sum();
    let len = sorted.len();

    LatencyStats {
        min: *sorted.first().unwrap(),
        max: *sorted.last().unwrap(),
        avg: sum / len as u32,
        p50: sorted[len * 50 / 100],
        p95: sorted[len * 95 / 100],
        p99: sorted[len * 99 / 100],
    }
}

fn print_stats(title: &str, stats: &LatencyStats) {
    println!("\n{}", title);
    println!("{}", "-".repeat(60));
    println!("  最小延迟 (Min):  {:>10.2?}", stats.min);
    println!("  平均延迟 (Avg):  {:>10.2?}", stats.avg);
    println!("  中位数 (P50):    {:>10.2?}", stats.p50);
    println!("  P95 延迟:        {:>10.2?}", stats.p95);
    println!("  P99 延迟:        {:>10.2?}", stats.p99);
    println!("  最大延迟 (Max):  {:>10.2?}", stats.max);
}
