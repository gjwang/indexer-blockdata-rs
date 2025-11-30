// Cargo.toml dependencies:
// [dependencies]
// rdkafka = { version = "0.36", features = ["cmake-build"] }
// tokio = { version = "1", features = ["full"] }
// serde = { version = "1.0", features = ["derive"] }
// serde_json = "1.0"
// anyhow = "1.0"
// chrono = "0.4"

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::Message;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct DemoMessage {
    id: u32,
    content: String,
}

#[derive(Debug, Clone)]
struct LatencyRecord {
    #[allow(dead_code)]
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

use fetcher::configure;

#[tokio::main]
async fn main() -> Result<()> {
    let config = configure::load_config().expect("Failed to load config");

    let broker = config.kafka_broker;
    let topic = config.kafka_topic;
    let group_id = format!(
        "{}-{}",
        config.kafka_group_id,
        chrono::Utc::now().timestamp()
    );

    println!("=== Redpanda Latency Test ===");
    println!("Topic: {}", topic);
    println!("Group ID: {}\n", group_id);

    let producer = create_producer(&broker)?;
    let consumer = create_consumer(&broker, &group_id)?;

    println!("Starting latency test with 100000 messages...\n");
    run_latency_test(&producer, &consumer, &topic, 100000).await?;

    Ok(())
}

fn create_producer(broker: &str) -> Result<FutureProducer> {
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("message.timeout.ms", "10000")
        .set("linger.ms", "0")
        .set("compression.type", "none")
        .set("acks", "1")
        .set("request.timeout.ms", "5000")
        .create()?;

    println!("Producer created successfully");
    Ok(producer)
}

fn create_consumer(broker: &str, group_id: &String) -> Result<StreamConsumer> {
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("group.id", group_id)
        .set("enable.auto.commit", "true")
        .set("auto.commit.interval.ms", "1000")
        .set("auto.offset.reset", "earliest")
        .set("fetch.min.bytes", "1")
        .set("session.timeout.ms", "10000")
        .set("heartbeat.interval.ms", "3000")
        .create()?;

    println!("Consumer created successfully");
    Ok(consumer)
}

async fn run_latency_test(
    producer: &FutureProducer,
    consumer: &StreamConsumer,
    topic: &str,
    message_count: usize,
) -> Result<()> {
    let send_times: Arc<Mutex<HashMap<u32, (Instant, Duration)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let latency_records: Arc<Mutex<Vec<LatencyRecord>>> = Arc::new(Mutex::new(Vec::new()));

    let _send_times_for_consumer = send_times.clone();
    let _latency_records_for_consumer = latency_records.clone();

    println!("Step 1: Starting consumer and subscribing to topic...");
    consumer.subscribe(&[topic])?;

    println!("Step 2: Waiting for consumer to be ready (1 seconds)...\n");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // println!("Step 3: Consumer is listening, starting to send messages...\n");
    // let consume_task = consume_messages_realtime(consumer, send_times_for_consumer, latency_records_for_consumer, message_count);

    let send_task = send_messages_realtime(producer, topic, send_times.clone(), message_count);

    tokio::join!(send_task);
    println!("\nStep 4: All tasks completed, calculating statistics...\n");

    let records = latency_records.lock().await;
    print_latency_stats(&records, message_count);

    Ok(())
}

async fn send_messages_realtime(
    producer: &FutureProducer,
    topic: &str,
    send_times: Arc<Mutex<HashMap<u32, (Instant, Duration)>>>,
    message_count: usize,
) {
    for i in 0..message_count {
        let msg = DemoMessage {
            id: i as u32,
            content: format!("Latency test message {}", i),
        };

        let payload = serde_json::to_string(&msg).unwrap();
        let key = format!("key-{}", i);

        let send_start = Instant::now();
        let record_msg = FutureRecord::to(topic).key(&key).payload(&payload);

        match producer.send(record_msg, Duration::from_secs(10)).await {
            Ok(_) => {
                let producer_latency = send_start.elapsed();
                send_times
                    .lock()
                    .await
                    .insert(i as u32, (send_start, producer_latency));

                if (i + 1) % 20 == 0 {
                    println!("  Sent {}/{} messages", i + 1, message_count);
                }
            }
            Err((e, _)) => {
                eprintln!("  Failed to send message {}: {:?}", i, e);
            }
        }

        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    println!("  Message sending completed");
}

#[allow(dead_code)]
async fn consume_messages_realtime(
    consumer: &StreamConsumer,
    send_times: Arc<Mutex<HashMap<u32, (Instant, Duration)>>>,
    latency_records: Arc<Mutex<Vec<LatencyRecord>>>,
    expected_count: usize,
) {
    let mut received_count = 0;
    let mut consecutive_timeouts = 0;
    let start_time = Instant::now();

    loop {
        if start_time.elapsed() > Duration::from_secs(30) {
            println!("  Consumer timeout after 30 seconds");
            break;
        }

        match tokio::time::timeout(Duration::from_millis(10000), consumer.recv()).await {
            Ok(Ok(msg)) => {
                consecutive_timeouts = 0;
                let receive_time = Instant::now();

                if let Some(payload) = msg.payload() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(demo_msg) = serde_json::from_str::<DemoMessage>(text) {
                            let send_times_map = send_times.lock().await;

                            if let Some((send_time, producer_latency)) =
                                send_times_map.get(&demo_msg.id)
                            {
                                let mut records = latency_records.lock().await;
                                records.push(LatencyRecord {
                                    msg_id: demo_msg.id,
                                    send_time: *send_time,
                                    receive_time: Some(receive_time),
                                    producer_latency: *producer_latency,
                                });

                                received_count += 1;

                                if received_count % 20 == 0 {
                                    println!(
                                        "  Received {}/{} messages",
                                        received_count, expected_count
                                    );
                                }

                                if received_count >= expected_count {
                                    println!("  All expected messages received");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("  Receive error: {:?}", e);
                consecutive_timeouts += 1;
            }
            Err(_) => {
                consecutive_timeouts += 1;
                if consecutive_timeouts >= 5 && received_count > 0 {
                    println!(
                        "  No messages for 5 seconds, stopping (received: {})",
                        received_count
                    );
                    break;
                }
            }
        }
    }
}

fn print_latency_stats(records: &[LatencyRecord], expected_count: usize) {
    let mut producer_latencies = Vec::new();
    let mut e2e_latencies = Vec::new();

    for record in records {
        producer_latencies.push(record.producer_latency);

        if let Some(receive_time) = record.receive_time {
            let e2e_latency = receive_time.duration_since(record.send_time);
            e2e_latencies.push(e2e_latency);
        }
    }

    println!("{}", "=".repeat(60));
    println!("Latency Test Results");
    println!("{}", "=".repeat(60));

    println!("\nTest Statistics:");
    println!("  Total messages sent:     {}", expected_count);
    println!("  Successfully received:   {}", records.len());
    println!(
        "  Loss rate:               {:.2}%",
        (expected_count - records.len()) as f64 / expected_count as f64 * 100.0
    );

    if !producer_latencies.is_empty() {
        let producer_stats = calculate_stats(&producer_latencies);
        print_stats("Producer Send Latency", &producer_stats);
    }

    if !e2e_latencies.is_empty() {
        let e2e_stats = calculate_stats(&e2e_latencies);
        print_stats("End-to-End Latency (Send -> Receive)", &e2e_stats);

        if !producer_latencies.is_empty() {
            let producer_stats = calculate_stats(&producer_latencies);
            let avg_network_consumer = e2e_stats.avg.saturating_sub(producer_stats.avg);
            println!(
                "\nNetwork + Consumer Latency (approx): {:?}",
                avg_network_consumer
            );
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
    println!("  Min Latency:     {:>10.2?}", stats.min);
    println!("  Avg Latency:     {:>10.2?}", stats.avg);
    println!("  Median (P50):    {:>10.2?}", stats.p50);
    println!("  P95 Latency:     {:>10.2?}", stats.p95);
    println!("  P99 Latency:     {:>10.2?}", stats.p99);
    println!("  Max Latency:     {:>10.2?}", stats.max);
}
