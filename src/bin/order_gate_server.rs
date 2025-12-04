use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher};
use fetcher::models::UserAccountManager;
use fetcher::symbol_manager::SymbolManager;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct KafkaPublisher(FutureProducer);

impl OrderPublisher for KafkaPublisher {
    fn publish(
        &self,
        topic: String,
        key: String,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let producer = self.0.clone();
        Box::pin(async move {
            let record = FutureRecord::to(&topic).payload(&payload).key(&key);
            producer
                .send(record, Duration::from_secs(0))
                .await
                .map(|_| ())
                .map_err(|(e, _)| e.to_string())
        })
    }
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    let symbol_manager = SymbolManager::load_from_db();

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", &config.kafka.linger_ms)
        .set("socket.keepalive.enable", &config.kafka.socket_keepalive_enable)
        .create()
        .expect("Producer creation error");

    // Connect to ScyllaDB
    let db = if let Some(scylla_config) = &config.scylladb {
        match fetcher::db::SettlementDb::connect(scylla_config).await {
            Ok(db) => {
                println!("✅ Connected to ScyllaDB");
                Some(Arc::new(db))
            }
            Err(e) => {
                eprintln!("⚠️ Warning: Failed to connect to ScyllaDB: {}", e);
                None
            }
        }
    } else {
        println!("⚠️ Warning: ScyllaDB config missing");
        None
    };

    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

    let state = Arc::new(AppState {
        symbol_manager,
        producer: Arc::new(KafkaPublisher(producer)),
        snowflake_gen,
        kafka_topic: config.kafka.topics.orders,
        user_manager: UserAccountManager::new(),
        db,
    });

    let app = create_app(state);

    println!("🚀 Order Gateway API running on http://localhost:3001");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
