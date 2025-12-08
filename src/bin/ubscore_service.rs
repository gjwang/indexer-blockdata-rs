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
use std::time::{Duration, Instant};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use rdkafka::producer::{FutureProducer, FutureRecord};
use tokio::sync::RwLock;

use fetcher::configure::{self, AppConfig};
use fetcher::logger::setup_logger;
use fetcher::ubs_core::{
    HealthChecker, HealthStatus, InternalOrder, LatencyTimer, OrderMetrics, RejectReason,
    SpotRiskModel, UBSCore,
};

const LOG_TARGET: &str = "ubscore";

// === CONFIGURATION ===
const STATS_INTERVAL_SECS: u64 = 10;
const HEALTH_HEARTBEAT_MS: u64 = 1000;

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

impl ServiceConfig {
    fn from_app_config(config: &AppConfig) -> Self {
        Self {
            kafka_brokers: config.kafka.broker.clone(),
            orders_topic: config.kafka.topics.orders.clone(),
            validated_orders_topic: "validated_orders".to_string(),
            consumer_group: config.kafka.group_id.clone(),
        }
    }
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
    metrics: Arc<OrderMetrics>,
    health: Arc<HealthChecker>,
}

impl UBSCoreService {
    fn new() -> Self {
        let core = UBSCore::new(SpotRiskModel);

        Self {
            core: RwLock::new(core),
            metrics: Arc::new(OrderMetrics::new()),
            health: Arc::new(HealthChecker::default()),
        }
    }

    /// Process an incoming order
    async fn process_order(&self, order: InternalOrder) -> Result<(), RejectReason> {
        let timer = LatencyTimer::start();

        self.metrics.record_received();

        let mut core = self.core.write().await;
        let result = core.process_order(order);

        let latency_ns = timer.elapsed_ns();

        match &result {
            Ok(()) => {
                self.metrics.record_accepted(latency_ns);
            }
            Err(reason) => {
                self.metrics.record_rejected(&format!("{:?}", reason), latency_ns);
            }
        }

        result
    }
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() {
    // --- Configuration ---
    let (app_config, service_config) = match configure::load_service_config("ubscore_config") {
        Ok(config) => {
            let svc_config = ServiceConfig::from_app_config(&config);
            (Some(config), svc_config)
        }
        Err(e) => {
            eprintln!("⚠️ Config load failed ({}), using defaults", e);
            (None, ServiceConfig::default())
        }
    };

    // Setup logger
    if let Some(ref config) = app_config {
        if let Err(e) = setup_logger(config) {
            eprintln!("Logger init failed: {}", e);
        }
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }

    log::info!(target: LOG_TARGET, "============================================");
    log::info!(target: LOG_TARGET, "       UBSCore Service Starting");
    log::info!(target: LOG_TARGET, "============================================");
    log::info!(target: LOG_TARGET, "Kafka brokers: {}", service_config.kafka_brokers);
    log::info!(target: LOG_TARGET, "Orders topic: {}", service_config.orders_topic);
    log::info!(target: LOG_TARGET, "Validated orders topic: {}", service_config.validated_orders_topic);
    log::info!(target: LOG_TARGET, "Consumer group: {}", service_config.consumer_group);

    // --- Create Service ---
    let service = Arc::new(UBSCoreService::new());
    log::info!(target: LOG_TARGET, "✅ UBSCore initialized");

    // --- Kafka Producer ---
    let producer: FutureProducer = match ClientConfig::new()
        .set("bootstrap.servers", &service_config.kafka_brokers)
        .set("message.timeout.ms", "5000")
        .create()
    {
        Ok(p) => {
            log::info!(target: LOG_TARGET, "✅ Kafka producer connected");
            p
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to create Kafka producer: {}", e);
            std::process::exit(1);
        }
    };

    // --- Kafka Consumer ---
    let consumer: StreamConsumer = match ClientConfig::new()
        .set("bootstrap.servers", &service_config.kafka_brokers)
        .set("group.id", &service_config.consumer_group)
        .set("enable.auto.commit", "true")
        .set("auto.offset.reset", "latest")
        .create()
    {
        Ok(c) => {
            log::info!(target: LOG_TARGET, "✅ Kafka consumer connected");
            c
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to create Kafka consumer: {}", e);
            std::process::exit(1);
        }
    };

    // Subscribe to orders topic
    if let Err(e) = consumer.subscribe(&[&service_config.orders_topic]) {
        log::error!(target: LOG_TARGET, "❌ Failed to subscribe: {}", e);
        std::process::exit(1);
    }
    log::info!(target: LOG_TARGET, "✅ Subscribed to topic: {}", service_config.orders_topic);

    // --- Mark Ready ---
    service.health.set_ready(true);
    service.health.heartbeat();
    log::info!(target: LOG_TARGET, "✅ UBSCore Service Ready");
    log::info!(target: LOG_TARGET, "============================================");

    // --- Spawn Stats Reporter ---
    let stats_service = service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(STATS_INTERVAL_SECS));
        let start_time = Instant::now();

        loop {
            interval.tick().await;

            let snapshot = stats_service.metrics.snapshot();
            let uptime = start_time.elapsed().as_secs();
            let health = stats_service.health.readiness();

            log::info!(
                target: LOG_TARGET,
                "[STATS] uptime={}s | recv={} accept={} reject={} | lat: avg={}µs min={}µs max={}µs | health={:?}",
                uptime,
                snapshot.orders_received,
                snapshot.orders_accepted,
                snapshot.orders_rejected,
                snapshot.avg_latency_us,
                snapshot.min_latency_us,
                snapshot.max_latency_us,
                match health {
                    HealthStatus::Healthy => "✅",
                    HealthStatus::Degraded(_) => "⚠️",
                    HealthStatus::Unhealthy(_) => "❌",
                }
            );
        }
    });

    // --- Spawn Heartbeat ---
    let health_clone = service.health.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(HEALTH_HEARTBEAT_MS));
        loop {
            interval.tick().await;
            health_clone.heartbeat();
        }
    });

    // --- Main Processing Loop ---
    run_processing_loop(service, consumer, producer, service_config).await;
}

// ============================================================================
// PROCESSING LOOP
// ============================================================================

async fn run_processing_loop(
    service: Arc<UBSCoreService>,
    consumer: StreamConsumer,
    producer: FutureProducer,
    config: ServiceConfig,
) {
    let mut total_processed: u64 = 0;
    let mut total_accepted: u64 = 0;
    let mut total_rejected: u64 = 0;
    let loop_start = Instant::now();

    log::info!(target: LOG_TARGET, "📥 Starting order processing loop...");

    loop {
        // --- Receive Order ---
        let recv_timer = LatencyTimer::start();

        match consumer.recv().await {
            Ok(msg) => {
                let recv_latency_us = recv_timer.elapsed_us();

                if let Some(payload) = msg.payload() {
                    let parse_timer = LatencyTimer::start();

                    // --- Parse Order ---
                    match bincode::deserialize::<InternalOrder>(payload) {
                        Ok(order) => {
                            let parse_latency_us = parse_timer.elapsed_us();
                            total_processed += 1;

                            log::debug!(
                                target: LOG_TARGET,
                                "[RECV] order_id={} user={} symbol={} side={:?} price={} qty={} | recv={}µs parse={}µs",
                                order.order_id,
                                order.user_id,
                                order.symbol_id,
                                order.side,
                                order.price,
                                order.qty,
                                recv_latency_us,
                                parse_latency_us
                            );

                            // --- Process Order ---
                            let process_timer = LatencyTimer::start();
                            let order_id = order.order_id;

                            match service.process_order(order.clone()).await {
                                Ok(()) => {
                                    let process_latency_us = process_timer.elapsed_us();
                                    total_accepted += 1;

                                    log::info!(
                                        target: LOG_TARGET,
                                        "[ACCEPT] order_id={} | process={}µs | total: proc={} accept={} reject={}",
                                        order_id,
                                        process_latency_us,
                                        total_processed,
                                        total_accepted,
                                        total_rejected
                                    );

                                    // --- Forward to ME ---
                                    let forward_timer = LatencyTimer::start();

                                    match bincode::serialize(&order) {
                                        Ok(validated_payload) => {
                                            let key_bytes = order_id.to_be_bytes();
                                            let record =
                                                FutureRecord::to(&config.validated_orders_topic)
                                                    .payload(&validated_payload)
                                                    .key(&key_bytes[..]);

                                            match producer
                                                .send(record, Duration::from_secs(1))
                                                .await
                                            {
                                                Ok((partition, offset)) => {
                                                    let forward_latency_us =
                                                        forward_timer.elapsed_us();
                                                    log::debug!(
                                                        target: LOG_TARGET,
                                                        "[FORWARD] order_id={} -> partition={} offset={} | forward={}µs",
                                                        order_id,
                                                        partition,
                                                        offset,
                                                        forward_latency_us
                                                    );
                                                }
                                                Err((e, _)) => {
                                                    log::error!(
                                                        target: LOG_TARGET,
                                                        "[FORWARD_ERROR] order_id={} error={:?}",
                                                        order_id,
                                                        e
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!(
                                                target: LOG_TARGET,
                                                "[SERIALIZE_ERROR] order_id={} error={}",
                                                order_id,
                                                e
                                            );
                                        }
                                    }
                                }
                                Err(reason) => {
                                    let process_latency_us = process_timer.elapsed_us();
                                    total_rejected += 1;

                                    log::warn!(
                                        target: LOG_TARGET,
                                        "[REJECT] order_id={} reason={:?} | process={}µs | total: proc={} accept={} reject={}",
                                        order_id,
                                        reason,
                                        process_latency_us,
                                        total_processed,
                                        total_accepted,
                                        total_rejected
                                    );

                                    // TODO: Send rejection response to Gateway
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                target: LOG_TARGET,
                                "[PARSE_ERROR] Failed to deserialize order: {} | payload_len={}",
                                e,
                                payload.len()
                            );
                        }
                    }
                }
            }
            Err(e) => {
                log::error!(target: LOG_TARGET, "[KAFKA_ERROR] {}", e);
            }
        }

        // --- Periodic Throughput Log ---
        if total_processed > 0 && total_processed % 1000 == 0 {
            let elapsed = loop_start.elapsed().as_secs_f64();
            let throughput = total_processed as f64 / elapsed;

            log::info!(
                target: LOG_TARGET,
                "[PROGRESS] processed={} accepted={} rejected={} | throughput={:.1} orders/sec",
                total_processed,
                total_accepted,
                total_rejected,
                throughput
            );
        }
    }
}
