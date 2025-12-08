//! UBSCore Service - Aeron UDP Version
//!
//! Receives orders from Gateway via Aeron UDP, validates, and sends responses.
//! Then forwards validated orders to Kafka for Matching Engine.
//!
//! # Architecture
//!
//! ```text
//! Gateway ──Aeron UDP (40456)──► UBSCore ──Kafka──► ME
//!    ◄────Aeron UDP (40457)────  (responses)
//! ```

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};

use fetcher::configure::{self, expand_tilde, AppConfig};
use fetcher::ubs_core::{
    GroupCommitConfig, GroupCommitWal, InternalOrder, OrderType, RejectReason, Side, SpotRiskModel,
    UBSCore, WalEntry, WalEntryType,
};

#[cfg(feature = "aeron")]
use fetcher::ubs_core::comm::{
    AeronConfig, OrderMessage, ResponseMessage, reason_codes,
};

#[cfg(feature = "aeron")]
use rusteron_client::*;

// Logging
const TARGET: &str = "UBSC";
macro_rules! info  { ($($arg:tt)*) => { log::info!(target: TARGET, $($arg)*) } }
macro_rules! warn  { ($($arg:tt)*) => { log::warn!(target: TARGET, $($arg)*) } }
macro_rules! error { ($($arg:tt)*) => { log::error!(target: TARGET, $($arg)*) } }

fn main() {
    // Setup logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    #[cfg(not(feature = "aeron"))]
    {
        eprintln!("❌ This binary requires --features aeron");
        std::process::exit(1);
    }

    #[cfg(feature = "aeron")]
    run_aeron_service();
}

#[cfg(feature = "aeron")]
fn run_aeron_service() {
    info!("🚀 UBSCore Service starting (Aeron mode)");

    // --- Initialize WAL ---
    let home = std::env::var("HOME").expect("HOME not set");
    let data_dir = PathBuf::from(home).join("ubscore_data");
    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");
    let wal_path = data_dir.join("ubscore.wal");

    let wal_config = GroupCommitConfig {
        buffer_size: 64 * 1024,
        max_batch_size: 100,
        use_direct_io: false,
    };
    let mut wal = GroupCommitWal::open(&wal_path, wal_config)
        .expect("Failed to open WAL");
    info!("✅ WAL opened at {:?}", wal_path);

    // --- Initialize UBSCore ---
    let mut ubs_core = UBSCore::new(SpotRiskModel);

    // Seed test accounts
    for user_id in 1001..=1010 {
        ubs_core.on_deposit(user_id, 1, 100_00000000);      // 100 BTC
        ubs_core.on_deposit(user_id, 2, 10_000_000_00000000); // 10M USDT
    }
    info!("✅ Seeded test accounts 1001-1010");

    // --- Launch Embedded Media Driver (for development) ---
    let _driver = fetcher::ubs_core::comm::EmbeddedDriver::launch()
        .expect("Failed to launch embedded media driver");
    info!("✅ Embedded Media Driver launched");

    // Give driver time to initialize
    std::thread::sleep(Duration::from_millis(500));

    // --- Initialize Aeron Client ---
    let config = AeronConfig::default();

    let ctx = AeronContext::new().expect("Failed to create Aeron context");

    // Set the same directory as the embedded driver
    use fetcher::ubs_core::comm::AERON_DIR;
    let dir_cstr = CString::new(AERON_DIR).unwrap();
    ctx.set_dir(&dir_cstr).expect("Failed to set aeron dir");
    info!("✅ Aeron client using dir: {}", AERON_DIR);

    let aeron = Aeron::new(&ctx).expect("Failed to create Aeron");
    aeron.start().expect("Failed to start Aeron");
    info!("✅ Aeron client started");

    // Subscription for receiving orders (Gateway → UBSCore)
    let orders_channel = CString::new(config.orders_channel.clone()).unwrap();
    let handler_avail: Option<&Handler<AeronAvailableImageLogger>> = None;
    let handler_unavail: Option<&Handler<AeronUnavailableImageLogger>> = None;

    let subscription = aeron
        .add_subscription(
            &orders_channel,
            config.orders_in_stream,
            handler_avail,
            handler_unavail,
            Duration::from_secs(5),
        )
        .expect("Failed to create subscription");
    info!("✅ Subscription created on {}", config.orders_channel);

    // Publication for sending responses (UBSCore → Gateway)
    let responses_channel = CString::new(config.responses_channel.clone()).unwrap();
    let publication = aeron
        .add_publication(&responses_channel, config.responses_out_stream, Duration::from_secs(5))
        .expect("Failed to create publication");
    info!("✅ Publication created on {}", config.responses_channel);

    // --- Initialize Kafka producer (for validated orders to ME) ---
    let kafka_producer: Option<FutureProducer> = match configure::load_config() {
        Ok(config) => {
            let producer: FutureProducer = ClientConfig::new()
                .set("bootstrap.servers", &config.kafka.broker)
                .set("message.timeout.ms", "5000")
                .create()
                .expect("Failed to create Kafka producer");
            info!("✅ Kafka producer created");
            Some(producer)
        }
        Err(e) => {
            warn!("⚠️ Kafka config not found ({}), orders won't be forwarded to ME", e);
            None
        }
    };

    info!("🎯 UBSCore Service ready - listening for orders");

    // --- Main processing loop ---
    let mut orders_received = 0u64;
    let mut orders_accepted = 0u64;
    let mut orders_rejected = 0u64;
    let mut last_stats = Instant::now();

    loop {
        // Poll for incoming orders
        let handler = OrderHandler {
            ubs_core: &mut ubs_core,
            wal: &mut wal,
            publication: &publication,
            kafka_producer: &kafka_producer,
            orders_received: &mut orders_received,
            orders_accepted: &mut orders_accepted,
            orders_rejected: &mut orders_rejected,
        };

        let handler_wrapped = Handler::leak(handler);
        let _ = subscription.poll(Some(&handler_wrapped), 10);

        // Print stats every 10 seconds
        if last_stats.elapsed() > Duration::from_secs(10) {
            info!(
                "[STATS] received={} accepted={} rejected={}",
                orders_received, orders_accepted, orders_rejected
            );
            last_stats = Instant::now();
        }

        // Small sleep to avoid busy-spin
        std::thread::sleep(Duration::from_micros(100));
    }
}

#[cfg(feature = "aeron")]
struct OrderHandler<'a> {
    ubs_core: &'a mut UBSCore<SpotRiskModel>,
    wal: &'a mut GroupCommitWal,
    publication: &'a AeronPublication,
    kafka_producer: &'a Option<FutureProducer>,
    orders_received: &'a mut u64,
    orders_accepted: &'a mut u64,
    orders_rejected: &'a mut u64,
}

#[cfg(feature = "aeron")]
impl<'a> AeronFragmentHandlerCallback for OrderHandler<'a> {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        *self.orders_received += 1;

        // Parse order message
        let order_msg = match OrderMessage::from_bytes(buffer) {
            Some(msg) => msg,
            None => {
                warn!("Invalid order message");
                return;
            }
        };

        // Convert to InternalOrder
        let order = match order_msg.to_internal_order() {
            Ok(o) => o,
            Err(e) => {
                warn!("Order conversion failed: {:?}", e);
                self.send_response(order_msg.order_id, false, reason_codes::INVALID_SYMBOL);
                *self.orders_rejected += 1;
                return;
            }
        };

        // Log to WAL
        if let Ok(payload) = bincode::serialize(&order) {
            let entry = WalEntry::new(WalEntryType::OrderLock, payload);
            if let Err(e) = self.wal.append(&entry) {
                error!("WAL append failed: {:?}", e);
                self.send_response(order.order_id, false, reason_codes::INTERNAL_ERROR);
                *self.orders_rejected += 1;
                return;
            }
        }

        // Process order
        match self.ubs_core.process_order(order.clone()) {
            Ok(()) => {
                // Flush WAL
                if let Err(e) = self.wal.flush() {
                    error!("WAL flush failed: {:?}", e);
                }

                // Send accept response
                self.send_response(order.order_id, true, 0);
                *self.orders_accepted += 1;

                // Forward to Kafka (async, best-effort)
                if let Some(producer) = self.kafka_producer {
                    let payload = bincode::serialize(&order).unwrap_or_default();
                    let key = order.order_id.to_string();
                    let record = FutureRecord::to("validated_orders")
                        .payload(&payload)
                        .key(&key);
                    let _ = producer.send(record, Duration::from_secs(0));
                }
            }
            Err(reason) => {
                let reason_code = match reason {
                    RejectReason::InsufficientBalance => reason_codes::INSUFFICIENT_BALANCE,
                    RejectReason::DuplicateOrderId => reason_codes::DUPLICATE_ORDER_ID,
                    RejectReason::AccountNotFound => reason_codes::ACCOUNT_NOT_FOUND,
                    _ => reason_codes::INTERNAL_ERROR,
                };
                self.send_response(order.order_id, false, reason_code);
                *self.orders_rejected += 1;
            }
        }
    }
}

#[cfg(feature = "aeron")]
impl<'a> OrderHandler<'a> {
    fn send_response(&self, order_id: u64, accepted: bool, reason_code: u8) {
        let resp = if accepted {
            ResponseMessage::accept(order_id)
        } else {
            ResponseMessage::reject(order_id, reason_code)
        };

        let bytes = resp.to_bytes();
        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let _ = self.publication.offer(bytes, handler);
    }
}
