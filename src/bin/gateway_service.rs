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
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
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
    let funding_account = Arc::new(AsyncMutex::new(fetcher::gateway::SimulatedFundingAccount::new()));
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

    // Clone TB client for Settlement Listener
    let tb_client_for_settlement = tb_client.clone();

    // --- Internal Transfer Initialization (FSM-based) ---
    let internal_transfer_enabled = std::env::var("INTERNAL_TRANSFER_ENABLED").is_ok();

    let (transfer_coordinator, transfer_worker, transfer_queue) = if internal_transfer_enabled {
        use fetcher::transfer::{
            TransferCoordinator, TransferWorker, TransferQueue, TransferDb, WorkerConfig,
            adapters::{TbFundingAdapter, TbTradingAdapter},
        };

        // Require both ScyllaDB and TigerBeetle for internal transfers
        match (&db, &tb_client) {
            (Some(ref session), Some(ref tb)) => {
                let transfer_db = Arc::new(TransferDb::new(session.get_session()));

                // Use real TigerBeetle adapters
                let funding_adapter = Arc::new(TbFundingAdapter::new(tb.clone()));
                let trading_adapter = Arc::new(TbTradingAdapter::new(tb.clone()));

                let coordinator = Arc::new(TransferCoordinator::new(
                    transfer_db.clone(),
                    funding_adapter,
                    trading_adapter,
                ));

                let queue = Arc::new(TransferQueue::new(10000));
                let config = WorkerConfig::default();

                let worker = Arc::new(TransferWorker::new(
                    coordinator.clone(),
                    transfer_db,
                    queue.clone(),
                    config,
                ));

                // Spawn background worker
                let worker_clone = worker.clone();
                tokio::spawn(async move {
                    worker_clone.run().await;
                });

                println!("✅ Internal Transfer ENABLED (TigerBeetle)");
                (Some(coordinator), Some(worker), Some(queue))
            }
            (None, _) => {
                println!("⚠️ Internal Transfer requires ScyllaDB connection");
                (None, None, None)
            }
            (_, None) => {
                println!("⚠️ Internal Transfer requires TigerBeetle connection");
                (None, None, None)
            }
        }
    } else {
        println!("⚠️ Internal Transfer DISABLED (set INTERNAL_TRANSFER_ENABLED=1 to enable)");
        (None, None, None)
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
        internal_transfer_db: internal_transfer_db.clone(),
        funding_account,
        #[cfg(feature = "aeron")]
        ubs_client,
        ubscore_timeout_ms: 5000,
        tb_client,
        transfer_coordinator,
        transfer_worker,
        transfer_queue,
    });

    // --- Internal Transfer Settlement Listener ---
    // DISABLED: The embedded Settlement Listener uses synchronous Kafka polling and TigerBeetle
    // operations which block the tokio runtime, causing HTTP handlers to become unresponsive.
    // Use the separate `internal_transfer_settlement` service instead.
    // See: cargo run --bin internal_transfer_settlement
    let _ = (internal_transfer_db.clone(), tb_client_for_settlement); // suppress unused warnings
    println!("⚠️ Embedded Settlement Listener DISABLED - use separate service");

    // --- Start HTTP Server ---
    let app = create_app(state);
    let port = 3001;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("🚀 Order Gateway running on {}", addr);

    axum::serve(
        tokio::net::TcpListener::bind(&addr).await.unwrap(),
        app.into_make_service(),
    )
    .await
    .unwrap();
}
