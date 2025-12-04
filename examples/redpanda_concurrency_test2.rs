use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// src/bin/redpanda_concurrency_test.rs
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
// use tokio;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let brokers = "localhost:9092";
    let topic = "test-topic";

    let producer: FutureProducer = rdkafka::ClientConfig::new()
        .set("bootstrap.servers", brokers)
        // .set("queue.buffering.max.messages", "10000") // 增加本地队列深度
        // .set("batch.size", "65536")                    // 增加批次大小 (64KB)
        // .set("linger.ms", "10")                        // 稍微等待以凑够批次
        // .set("compression.type", "lz4")                // Redpanda 推荐使用 LZ4 或 Zstd
        // .set("acks", "1")                              // 1: Leader ack (折中), all: 最安全, 0: 最快
        .create()?;

    let num_messages = 1_000_000;
    let concurrency = 1000;

    // 📊 统计指标（原子计数）
    let total_sent = Arc::new(AtomicUsize::new(0));
    let success_count = Arc::new(AtomicUsize::new(0));
    let error_count = Arc::new(AtomicUsize::new(0));

    let start_time = Instant::now();

    let mut tasks = Vec::with_capacity(num_messages);

    for i in 0..concurrency {
        let producer = producer.clone();
        let topic = topic.to_string();
        let total_sent = total_sent.clone();
        let success_count = success_count.clone();
        let error_count = error_count.clone();

        let task = tokio::spawn(async move {
            for j in 0..(num_messages / concurrency) {
                total_sent.fetch_add(1, Ordering::Relaxed);

                let key = format!("key-{}", i);
                let payload = format!("msg-{}-{}", i, j);
                let record = FutureRecord::to(&topic).payload(&payload).key(&key);

                match producer.send(record, Timeout::After(Duration::from_secs(10))).await {
                    Ok(_) => {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err((e, _)) => {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        eprintln!("Send error: {}", e);
                    }
                }
            }
        });
        tasks.push(task);
    }

    // 等待所有任务完成
    for task in tasks {
        task.await?;
    }

    let elapsed = start_time.elapsed();
    let total = total_sent.load(Ordering::Relaxed);
    let success = success_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let throughput = (total as f64) / elapsed.as_secs_f64();

    // 📈 打印统计结果
    println!("========== Redpanda Concurrency Test Report ==========");
    println!("Total messages attempted: {}", total);
    println!("✅ Successfully sent:      {}", success);
    println!("❌ Failed to send:         {}", errors);
    println!("⏱️  Total time:           {:.2?}", elapsed);
    println!("🚀 Throughput:            {:.2} msg/sec", throughput);
    println!("=======================================================");

    if errors > 0 {
        eprintln!("⚠️  Warning: {} messages failed to send.", errors);
        std::process::exit(1); // 可选：失败时退出码非0，便于 CI
    }

    Ok(())
}
