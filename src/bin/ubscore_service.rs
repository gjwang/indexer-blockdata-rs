//! UBSCore Service - User Balance Service Core
//!
//! This service sits between Gateway and Matching Engine:
//! - Receives orders from Gateway (via Kafka)
//! - Validates orders (balance check, dedup, risk)
//! - Forwards validated orders to Matching Engine
//! - Receives trade fills and updates balances
//!
//! # Architecture
//!
//! ```text
//! Gateway → [Kafka:orders] → UBSCore → [Kafka:validated_orders] → ME
//!                              ↑
//!         ME → [Kafka:fills] ─┘
//! ```

use std::sync::Arc;
use std::time::Duration;

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use tokio::sync::RwLock;

use fetcher::ubs_core::{InternalOrder, RejectReason, SpotRiskModel, UBSCore};

/// UBSCore Service configuration
#[derive(Debug, Clone)]
struct ServiceConfig {
    /// Kafka broker address
    kafka_brokers: String,

    /// Topic for incoming orders from Gateway
    orders_topic: String,

    /// Topic for validated orders to ME
    validated_orders_topic: String,

    /// Consumer group ID
    consumer_group: String,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            kafka_brokers: "localhost:9092".to_string(),
            orders_topic: "orders".to_string(),
            validated_orders_topic: "validated_orders".to_string(),
            consumer_group: "ubscore".to_string(),
        }
    }
}

/// UBSCore Service state
struct UBSCoreService {
    core: RwLock<UBSCore<SpotRiskModel>>,
    config: ServiceConfig,
}

impl UBSCoreService {
    fn new(config: ServiceConfig) -> Self {
        let core = UBSCore::new(SpotRiskModel);

        Self { core: RwLock::new(core), config }
    }

    /// Process an incoming order
    async fn process_order(&self, order: InternalOrder) -> Result<(), RejectReason> {
        let mut core = self.core.write().await;
        core.process_order(order)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup simple logging via env_logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("Starting UBSCore Service...");

    // Use default config
    let config = ServiceConfig::default();

    log::info!("Kafka brokers: {}", config.kafka_brokers);
    log::info!("Orders topic: {}", config.orders_topic);
    log::info!("Validated orders topic: {}", config.validated_orders_topic);

    // Create service
    let service = Arc::new(UBSCoreService::new(config.clone()));

    // Create Kafka producer
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka_brokers)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Producer creation failed");

    // Create Kafka consumer
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka_brokers)
        .set("group.id", &config.consumer_group)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .create()
        .expect("Consumer creation failed");

    // Subscribe to orders topic
    consumer.subscribe(&[&config.orders_topic]).expect("Subscription failed");

    log::info!("Subscribed to topic: {}", config.orders_topic);
    log::info!("UBSCore Service ready. Press Ctrl+C to stop.");

    // Main processing loop
    loop {
        match consumer.recv().await {
            Ok(msg) => {
                if let Some(payload) = msg.payload() {
                    // Deserialize order
                    if let Ok(order) = bincode::deserialize::<InternalOrder>(payload) {
                        log::debug!(
                            "Received order: id={}, user={}, symbol={}",
                            order.order_id,
                            order.user_id,
                            order.symbol_id
                        );

                        // Process order
                        match service.process_order(order.clone()).await {
                            Ok(()) => {
                                log::info!("Order {} accepted", order.order_id);

                                // Forward to ME
                                let validated_payload = bincode::serialize(&order)?;
                                let key_bytes = order.order_id.to_be_bytes();
                                let record = FutureRecord::to(&config.validated_orders_topic)
                                    .payload(&validated_payload)
                                    .key(&key_bytes[..]);

                                if let Err((e, _)) =
                                    producer.send(record, Duration::from_secs(1)).await
                                {
                                    log::error!("Failed to forward order: {:?}", e);
                                }
                            }
                            Err(reason) => {
                                log::warn!("Order {} rejected: {:?}", order.order_id, reason);
                                // TODO: Send rejection response to Gateway
                            }
                        }
                    } else {
                        log::warn!("Failed to deserialize order message");
                    }
                }
            }
            Err(e) => {
                log::error!("Kafka error: {}", e);
            }
        }
    }
}
