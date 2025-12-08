//! UBSCore Test Order Producer
//!
//! Sends test orders to Kafka for testing the UBSCore service.
//!
//! Usage:
//!   cargo run --bin ubscore_test_producer -- [count] [rate]
//!
//! Examples:
//!   cargo run --bin ubscore_test_producer -- 10        # Send 10 orders, 1/sec
//!   cargo run --bin ubscore_test_producer -- 100 10    # Send 100 orders, 10/sec
//!   cargo run --bin ubscore_test_producer -- 1000 0    # Send 1000 orders as fast as possible

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::ubs_core::{InternalOrder, OrderType, Side};

const KAFKA_BROKER: &str = "localhost:9093";
const ORDERS_TOPIC: &str = "orders";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let count: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let rate: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    println!("============================================");
    println!("     UBSCore Test Order Producer");
    println!("============================================");
    println!("Kafka broker: {}", KAFKA_BROKER);
    println!("Topic: {}", ORDERS_TOPIC);
    println!("Orders to send: {}", count);
    println!("Rate: {} orders/sec (0 = unlimited)", rate);
    println!("============================================");

    // Create Kafka producer
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", KAFKA_BROKER)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Failed to create producer");

    println!("✅ Connected to Kafka");
    println!();

    let start = Instant::now();
    let mut sent = 0u64;
    let mut errors = 0u64;

    for i in 0..count {
        // Generate order
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;

        let order_id = SnowflakeGenRng::from_parts(now_ms, 1, (i % 8192) as u16);

        let order = InternalOrder {
            order_id,
            user_id: i % 1000, // Various user IDs
            symbol_id: 1,      // BTC/USDT
            side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
            price: 50000_00000000 + (i % 100) * 1_00000000, // 50000-50099 USDT
            qty: 1_00000000 + (i % 10) * 10000000,          // 1.0 - 1.9 BTC
            order_type: OrderType::Limit,
        };

        // Serialize using bincode
        let payload = bincode::serialize(&order)?;
        let key = order_id.to_be_bytes();

        // Send to Kafka
        let record = FutureRecord::to(ORDERS_TOPIC).payload(&payload).key(&key[..]);

        match producer.send(record, Duration::from_secs(1)).await {
            Ok((partition, offset)) => {
                sent += 1;
                println!(
                    "[{}] Sent order_id={} user={} side={:?} price={} qty={} -> p{}:{}",
                    sent,
                    order_id,
                    order.user_id,
                    order.side,
                    order.price,
                    order.qty,
                    partition,
                    offset
                );
            }
            Err((e, _)) => {
                errors += 1;
                eprintln!("[ERROR] Failed to send order: {:?}", e);
            }
        }

        // Rate limiting
        if rate > 0 && i < count - 1 {
            let delay_ms = 1000 / rate;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    let elapsed = start.elapsed();
    let throughput = sent as f64 / elapsed.as_secs_f64();

    println!();
    println!("============================================");
    println!("              Summary");
    println!("============================================");
    println!("Sent: {} orders", sent);
    println!("Errors: {}", errors);
    println!("Elapsed: {:.2}s", elapsed.as_secs_f64());
    println!("Throughput: {:.1} orders/sec", throughput);
    println!("============================================");

    Ok(())
}
