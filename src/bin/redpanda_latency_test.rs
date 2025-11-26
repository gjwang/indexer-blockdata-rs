use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::{Deserialize, Serialize};
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

    println!("=== Redpanda Latency Test ===\n");

    let producer = create_producer(broker)?;
    let consumer = create_consumer(broker, group_id)?;
    consumer.subscribe(&[topic])?;

    println!("Starting latency test...\n");
    run_latency_test(&producer, &consumer, topic, 100).await?;

    Ok(())
}

fn create_producer(broker: &str) -> Result<FutureProducer> {
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", "0")
        .set("compression.type", "none")
        .set("acks", "all")
        .create()?;

    println!("Producer created successfully");
    Ok(producer)
}

fn create_consumer(broker: &str, group_id: &str) -> Result<StreamConsumer> {
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("group.id", group_id)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .set("fetch.min.bytes", "1")
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
    let latency_records = Arc::new(Mutex::new(Vec::<LatencyRecord>::new()));
    let records_for_consumer = latency_records.clone();
    let records_for_send = latency_records.clone();

    println!("Consumer listening...");

    tokio::time::sleep(Duration::from_secs(1)).await;

    println!("Sending {} test messages...\n", message_count);

    let send_task = async { send_messages(producer, topic, records_for_send, message_count).await };

    let receive_task =
        async { consume_messages(consumer, records_for_consumer, message_count).await };

    tokio::join!(send_task, receive_task);

    println!("\nTest completed, calculating statistics...\n");

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
    for i in 0..message_count {
        let msg = DemoMessage {
            id: i as u32,
            content: format!("Latency test message {}", i),
        };

        let payload = serde_json::to_string(&msg).unwrap();
        let key = format!("key-{}", i);

        let send_start = Instant::now();
        let record_msg = FutureRecord::to(topic).key(&key).payload(&payload);

        match producer.send(record_msg, Duration::from_secs(5)).await {
            Ok(_) => {
                let producer_latency = send_start.elapsed();

                let mut recs = records.lock().await;
                recs.push(LatencyRecord {
                    msg_id: i as u32,
                    send_time: send_start,
                    receive_time: None,
                    producer_latency,
                });

                if (i + 1) % 20 == 0 {
                    println!("  Sent {}/{} messages", i + 1, message_count);
                }
            }
            Err((e, _)) => {
                eprintln!("  Failed to send message {}: {:?}", i, e);
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    println!("\nMessage sending completed");
}

async fn consume_messages(
    consumer: &StreamConsumer,
    records: Arc<Mutex<Vec<LatencyRecord>>>,
    expected_count: usize,
) {
    let mut received_count = 0;
    println!("Consumer started receiving messages...\n");

    loop {
        match tokio::time::timeout(Duration::from_secs(5), consumer.recv()).await {
            Ok(Ok(msg)) => {
                let receive_time = Instant::now();

                if let Some(payload) = msg.payload() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(demo_msg) = serde_json::from_str::<DemoMessage>(text) {
                            let mut recs = records.lock().await;
                            if let Some(record) = recs.iter_mut().find(|r| r.msg_id == demo_msg.id)
                            {
                                record.receive_time = Some(receive_time);
                                received_count += 1;

                                if received_count % 20 == 0 {
                                    println!(
                                        "  Received {}/{} messages",
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
                eprintln!("  Receive error: {:?}", e);
            }
            Err(_) => {
                println!("  Receive timeout, received {} messages", received_count);
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
    println!("Latency Test Results");
    println!("{}", "=".repeat(60));

    println!("\nTest Statistics:");
    println!("  Total messages sent:     {}", records.len());
    println!("  Successfully received:   {}", received_count);
    println!(
        "  Loss rate:               {:.2}%",
        (records.len() - received_count) as f64 / records.len() as f64 * 100.0
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

fn calculate_stats(latencies: &Vec<Duration>) -> LatencyStats {
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
