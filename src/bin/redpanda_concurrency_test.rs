use std::sync::Arc;
use std::time::Instant;

use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use tokio::sync::Semaphore;

// 配置参数
const BROKERS: &str = "localhost:9092"; // Redpanda 地址
const TOPIC: &str = "bench-test";
const MSG_COUNT: usize = 1_000_000;      // 发送一百万条
const MSG_SIZE: usize = 1024;            // 1KB 消息
const MAX_INFLIGHT: usize = 10_000;      // 限制并发数，防止内存溢出

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // 1. 创建 Producer 配置
    // 针对 Redpanda/Kafka 高吞吐的调优参数
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", BROKERS)
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "100000") // 增加本地队列深度
        .set("batch.size", "65536")                    // 增加批次大小 (64KB)
        .set("linger.ms", "10")                        // 稍微等待以凑够批次
        .set("compression.type", "lz4")                // Redpanda 推荐使用 LZ4 或 Zstd
        .set("acks", "1")                              // 1: Leader ack (折中), all: 最安全, 0: 最快
        .create()
        .expect("Producer creation error");

    println!("开始向 Redpanda 发送 {} 条消息...", MSG_COUNT);
    
    // 信号量：用于控制并发，防止生产速度远超网络发送速度导致 OOM
    let semaphore = Arc::new(Semaphore::new(MAX_INFLIGHT));

    let start_time = Instant::now();
    let mut tasks = Vec::with_capacity(MSG_COUNT);

    // 在循环外定义 payload (只读引用)
    // 实际上 rdkafka 的 payload 可以是 &[u8]，但因为我们要 move 到 spawn 里，
    // 最好的办法是所有任务共享同一块内存。
    // 在 Rust 中，要在多线程/多任务间共享只读数据，使用 Arc。
    let payload = Arc::new(vec![0u8; MSG_SIZE]);

    for i in 0..MSG_COUNT {
        let producer = producer.clone();
        let payload = payload.clone(); // Arc 的 clone 极快，只是增加引用计数，不拷贝数据
        let permit = semaphore.clone().acquire_owned().await.unwrap();

        let task = tokio::spawn(async move {
            let key = format!("key-{}", i);

            let record = FutureRecord::to(TOPIC)
                .payload(&payload[..]) // 获取 Arc 内部数据的切片引用
                .key(&key);

            let _ = producer.send(record, std::time::Duration::from_secs(0)).await;

            drop(permit);
        });
        tasks.push(task);
    }

    // 等待所有任务完成
    for task in tasks {
        let _ = task.await;
    }

    let duration = start_time.elapsed();
    let seconds = duration.as_secs_f64();
    let throughput = MSG_COUNT as f64 / seconds;
    let mb_per_sec = (MSG_COUNT * MSG_SIZE) as f64 / 1024.0 / 1024.0 / seconds;

    println!("--------------------------------------------------");
    println!("测试完成!");
    println!("耗时: {:.2} 秒", seconds);
    println!("吞吐量 (TPS): {:.2} msgs/sec", throughput);
    println!("带宽: {:.2} MB/sec", mb_per_sec);
    println!("--------------------------------------------------");

    Ok(())
}