use clap::Parser;
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::models::OrderRequest;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;
use tokio::time;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "localhost:9092")]
    brokers: String,

    #[arg(short, long, default_value = "orders")]
    topic: String,

    #[arg(short, long, default_value_t = 1000)]
    count: usize,

    #[arg(short, long, default_value_t = 100)]
    interval_ms: u64,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &args.brokers)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Producer creation error");

    let mut snowflake_gen = SnowflakeGenRng::new(1);
    let symbols = vec!["BTC_USDT", "ETH_USDT"];

    println!(">>> Starting Order Gateway (Producer)");
    println!(">>> Target: {}, Topic: {}", args.brokers, args.topic);

    for i in 0..args.count {
        let order_id = snowflake_gen.generate();
        let symbol = symbols[i % symbols.len()].to_string();
        let side = if i % 2 == 0 { "Buy" } else { "Sell" }.to_string();
        let price = 50000 + (i as u64 % 100); // Realistic BTC price
        let quantity = 1 + (i as u64 % 5);
        let user_id = 1000 + (i as u64 % 10);

        let order = OrderRequest::PlaceOrder {
            order_id,
            user_id,
            symbol: symbol.clone(),
            side,
            price,
            quantity,
            order_type: "Limit".to_string(),
        };

        let payload = serde_json::to_string(&order).unwrap();
        let key = order_id.to_string();

        let record = FutureRecord::to(&args.topic)
            .payload(&payload)
            .key(&key);

        match producer.send(record, Duration::from_secs(0)).await {
            Ok((partition, offset)) => println!("Sent Order {}: Partition {}, Offset {}", order_id, partition, offset),
            Err((e, _)) => eprintln!("Error sending order {}: {:?}", order_id, e),
        }

        time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
}
