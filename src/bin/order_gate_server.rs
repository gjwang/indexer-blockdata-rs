//! Order Gateway Server
//!
//! HTTP API → Aeron (UBSCore) → Kafka (ME)
//!
//! Architecture:
//! - HTTP handlers send orders directly to UBSCore via async Aeron client
//! - UBSCore validates and responds
//! - Gateway returns accept/reject to client

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::models::{balance_manager, UserAccountManager};
use fetcher::symbol_manager::SymbolManager;

use fetcher::gateway::{create_app, AppState};
use fetcher::gateway::OrderPublisher;
use fetcher::logging::setup_async_file_logging;

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
                .map_err(|(e, _)| format!("Kafka send failed: {:?}", e))
        })
    }
}

#[tokio::main]
async fn main() {
    // Phase 3: Async logging with JSON + daily rotation
    let _guard = setup_async_file_logging("gateway", "logs");

    tracing::info!("🚪 Gateway starting with async JSON logging");

    let config = fetcher::configure::load_config().expect("Failed to load config");

    let symbol_manager = Arc::new(SymbolManager::load_from_db());

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

                // (Optional) MV check removed as we use TigerBeetle now

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

    // --- Setup UBS async client ---
    #[cfg(feature = "aeron")]
    let ubs_client = {
        use fetcher::ubs_core::comm::{AeronConfig, UbsGatewayClient};

        let mut client = UbsGatewayClient::new(AeronConfig::default());

        // Connect to Aeron
        if let Err(e) = client.connect() {
            eprintln!("❌ Failed to connect to UBSCore via Aeron: {:?}", e);
            std::process::exit(1);
        }
        println!("✅ Connected to UBSCore via Aeron UDP");

        Arc::new(client)
    };

    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));
    let funding_account = Arc::new(Mutex::new(fetcher::gateway::SimulatedFundingAccount::new()));
    let balance_topic =
        config.kafka.topics.balance_ops.as_ref().unwrap_or(&"balance_ops".to_string()).clone();

    // Use validated_orders topic for approved orders → ME
    let validated_orders_topic = "validated_orders".to_string();

    let balance_manager = balance_manager::BalanceManager::new(symbol_manager.clone());

    // --- Connect to TigerBeetle ---
    use tigerbeetle_unofficial::Client as TigerBeetleClient;
    let tb_client = match TigerBeetleClient::new(0, "127.0.0.1:3000") {
        Ok(c) => {
            println!("✅ Connected to TigerBeetle");
            Some(Arc::new(c))
        },
        Err(e) => {
            eprintln!("⚠️ Warning: Failed to connect to TigerBeetle: {:?}", e);
            None
        }
    };

    // --- Connect to Internal Transfer DB ---
    // Try to connect separately to ensure internal transfer functionality works even if settlement DB has schema issues
    let internal_transfer_db = if let Some(scylla_config) = &config.scylladb {
        let session = scylla::SessionBuilder::new()
            .known_nodes(&scylla_config.hosts)
            .build()
            .await;

        match session {
            Ok(s) => {
                println!("✅ Connected to ScyllaDB for Internal Transfers");
                let db = fetcher::db::InternalTransferDb::new(Arc::new(s));
                Some(Arc::new(db))
            },
            Err(e) => {
                eprintln!("⚠️ Warning: Failed to connect to ScyllaDB for Internal Transfers: {}", e);
                None
            }
        }
    } else {
        None
    };

    let state = Arc::new(AppState {
        symbol_manager,
        balance_manager,
        producer: Arc::new(KafkaPublisher(producer)),
        snowflake_gen,
        kafka_topic: validated_orders_topic,
        balance_topic,
        user_manager: UserAccountManager::new(),
        db: db.map(|d| (*d).clone()),
        internal_transfer_db,
        funding_account,
        #[cfg(feature = "aeron")]
        ubs_client,
        ubscore_timeout_ms: 5000,
        tb_client,
    });

    let app = create_app(state);

    println!("🚀 Order Gateway API running on http://localhost:3001");
    println!("   Flow: HTTP → Aeron (UBSCore) → Kafka (ME)");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
