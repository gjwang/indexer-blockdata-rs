use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use tokio::sync::RwLock;

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher};
use fetcher::models::{balance_manager, UserAccountManager};
use fetcher::symbol_manager::SymbolManager;
use fetcher::ubs_core::{SpotRiskModel, UBSCore};

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
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

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

    // --- Initialize UBSCore with WAL (embedded in Gateway) ---
    let home = std::env::var("HOME").expect("HOME not set");
    let data_dir = std::path::PathBuf::from(home).join("gateway_data");
    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");
    let wal_path = data_dir.join("gateway_ubs.wal");

    let mut ubs_core = UBSCore::with_wal(SpotRiskModel, &wal_path)
        .expect("Failed to initialize UBSCore with WAL");
    println!("✅ UBSCore WAL at {:?}", wal_path);

    // Seed test accounts (development only)
    // TODO: Replace with proper balance sync from Settlement
    for user_id in 1001..=1010 {
        ubs_core.on_deposit(user_id, 1, 100_00000000);      // 100 BTC
        ubs_core.on_deposit(user_id, 2, 10_000_000_00000000); // 10M USDT
    }
    println!("✅ UBSCore initialized with test accounts 1001-1010");

    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));
    let funding_account = Arc::new(Mutex::new(fetcher::gateway::SimulatedFundingAccount::new()));
    let balance_topic =
        config.kafka.topics.balance_ops.as_ref().unwrap_or(&"balance_ops".to_string()).clone();

    // Use validated_orders topic for approved orders → ME
    let validated_orders_topic = "validated_orders".to_string();

    let symbol_manager = Arc::new(symbol_manager);
    let balance_manager = balance_manager::BalanceManager::new(symbol_manager.clone());

    let state = Arc::new(AppState {
        symbol_manager,
        balance_manager,
        producer: Arc::new(KafkaPublisher(producer)),
        snowflake_gen,
        kafka_topic: validated_orders_topic,  // Approved orders → ME
        balance_topic,
        user_manager: UserAccountManager::new(),
        db: db.map(|d| (*d).clone()),
        funding_account,
        ubs_core: Arc::new(RwLock::new(ubs_core)),
    });

    let app = create_app(state);

    println!("🚀 Order Gateway API running on http://localhost:3001");
    println!("   Flow: HTTP → UBSCore (sync) → Kafka(validated_orders) → ME");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
