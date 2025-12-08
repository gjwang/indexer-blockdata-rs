//! Order Gateway Server
//!
//! HTTP API → Aeron (UBSCore) → Kafka (ME)
//!
//! Architecture:
//! - Gateway HTTP handlers send orders via channel to UBS handler thread
//! - UBS handler thread uses Aeron to communicate with UBSCore service
//! - UBSCore service validates and responds
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

#[cfg(feature = "aeron")]
use fetcher::gateway::{create_app, AppState, UbsOrderRequest, UbsOrderResponse};

#[cfg(not(feature = "aeron"))]
use fetcher::gateway::{create_app, AppState};

use fetcher::gateway::OrderPublisher;

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

    // --- Setup UBS communication channel ---
    #[cfg(feature = "aeron")]
    let ubs_order_tx = {
        use fetcher::ubs_core::comm::{AeronConfig, UbsGatewayClient};

        // Create bounded channel for order requests
        let (tx, rx) = std::sync::mpsc::sync_channel::<UbsOrderRequest>(1000);

        // Spawn UBS handler thread (dedicated thread for Aeron)
        std::thread::spawn(move || {
            let mut client = UbsGatewayClient::new(AeronConfig::default());

            // Connect to Aeron
            if let Err(e) = client.connect() {
                eprintln!("❌ Failed to connect to UBSCore via Aeron: {:?}", e);
                return;
            }
            println!("✅ Connected to UBSCore via Aeron UDP");

            // Process orders from channel
            while let Ok(request) = rx.recv() {
                let response = match client.send_order_and_wait(&request.order, 100) {
                    Ok(resp) => UbsOrderResponse {
                        accepted: resp.is_accepted(),
                        reason_code: resp.reason_code,
                    },
                    Err(e) => {
                        log::error!("[UBS_HANDLER] Aeron error: {:?}", e);
                        UbsOrderResponse {
                            accepted: false,
                            reason_code: 99, // Internal error
                        }
                    }
                };

                // Send response back (ignore errors if receiver dropped)
                let _ = request.response_tx.send(response);
            }

            println!("UBS handler thread exiting");
        });

        tx
    };

    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));
    let funding_account = Arc::new(Mutex::new(fetcher::gateway::SimulatedFundingAccount::new()));
    let balance_topic =
        config.kafka.topics.balance_ops.as_ref().unwrap_or(&"balance_ops".to_string()).clone();

    // Use validated_orders topic for approved orders → ME
    let validated_orders_topic = "validated_orders".to_string();

    let balance_manager = balance_manager::BalanceManager::new(symbol_manager.clone());

    let state = Arc::new(AppState {
        symbol_manager,
        balance_manager,
        producer: Arc::new(KafkaPublisher(producer)),
        snowflake_gen,
        kafka_topic: validated_orders_topic,
        balance_topic,
        user_manager: UserAccountManager::new(),
        db: db.map(|d| (*d).clone()),
        funding_account,
        #[cfg(feature = "aeron")]
        ubs_order_tx,
    });

    let app = create_app(state);

    println!("🚀 Order Gateway API running on http://localhost:3001");
    println!("   Flow: HTTP → Aeron (UBSCore) → Kafka (ME)");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
