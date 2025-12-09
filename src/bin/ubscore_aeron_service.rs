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
    MmapWal, SpotRiskModel, UBSCore, install_sigbus_handler,
};

#[cfg(feature = "aeron")]
use fetcher::ubs_core::comm::{
    AeronConfig, UbsCoreHandler, HandlerStats, parse_request,
};

#[cfg(feature = "aeron")]
use rusteron_client::*;

// Logging
const TARGET: &str = "UBSC";
macro_rules! info  { ($($arg:tt)*) => { log::info!(target: TARGET, $($arg)*) } }
macro_rules! warn  { ($($arg:tt)*) => { log::warn!(target: TARGET, $($arg)*) } }
macro_rules! error { ($($arg:tt)*) => { log::error!(target: TARGET, $($arg)*) } }

fn init_env_logger() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
}

fn main() {
    // Try config-based logging, fallback to env_logger
    if let Ok(config) = configure::load_service_config("ubscore_config") {
        if fetcher::logger::setup_logger(&config).is_err() {
            init_env_logger();
        }
    } else {
        init_env_logger();
    }

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

    // --- Install SIGBUS handler for mmap safety ---
    // Catches disk full/I/O errors gracefully instead of silent crash
    install_sigbus_handler();

    // --- Initialize WAL (mmap-based - 10x faster on macOS APFS) ---
    // msync bypasses APFS transaction layer → ~500µs vs ~5ms for fdatasync
    // Durability verified: data survives process crash (crash test passed)
    // See docs/WAL_PERFORMANCE.md for detailed benchmarks
    let home = std::env::var("HOME").expect("HOME not set");
    let data_dir = PathBuf::from(home).join("ubscore_data");
    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");
    let wal_path = data_dir.join("ubscore_mmap.wal");

    let mut wal = MmapWal::open(&wal_path).expect("Failed to open WAL");
    info!("✅ MmapWal opened at {:?}", wal_path);

    // --- Initialize UBSCore ---
    let mut ubs_core = UBSCore::new(SpotRiskModel);

    // Seed test accounts (optional - can be done via Deposit messages from Gateway)
    // Set SEED_TEST_ACCOUNTS=0 to disable
    if std::env::var("SEED_TEST_ACCOUNTS").unwrap_or("1".into()) != "0" {
        for user_id in 1001..=1010 {
            ubs_core.on_deposit(user_id, 1, 100_00000000);      // 100 BTC
            ubs_core.on_deposit(user_id, 2, 10_000_000_00000000); // 10M USDT
        }
        info!("✅ Seeded test accounts 1001-1010 (disable with SEED_TEST_ACCOUNTS=0)");
    } else {
        info!("ℹ️ Test account seeding disabled - use Deposit messages");
    }

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
    let mut stats = HandlerStats::new();
    let mut last_stats = Instant::now();
    let mut last_received = 0u64;

    loop {
        // Create handler with clean separation
        let business_handler = UbsCoreHandler {
            ubs_core: &mut ubs_core,
            wal: &mut wal,
            kafka_producer: &kafka_producer,
            stats: &mut stats,
        };

        let transport_handler = OrderHandler {
            handler: business_handler,
            publication: &publication,
        };

        let handler_wrapped = Handler::leak(transport_handler);
        let _ = subscription.poll(Some(&handler_wrapped), 10);

        // Print stats every 10 seconds
        if last_stats.elapsed() > Duration::from_secs(10) {
            let elapsed_secs = last_stats.elapsed().as_secs_f64();
            let orders_in_period = stats.received - last_received;
            let qps = orders_in_period as f64 / elapsed_secs;

            if stats.received > 0 && stats.latency_min_us < u64::MAX {
                let avg_us = stats.latency_sum_us / stats.received;
                info!(
                    "[STATS] received={} accepted={} rejected={} qps={:.1} latency(µs): min={} avg={} max={}",
                    stats.received, stats.accepted, stats.rejected, qps, stats.latency_min_us, avg_us, stats.latency_max_us
                );
            } else {
                info!(
                    "[STATS] received={} accepted={} rejected={} qps={:.1}",
                    stats.received, stats.accepted, stats.rejected, qps
                );
            }
            last_stats = Instant::now();
            last_received = stats.received;
        }

        // Small sleep to avoid busy-spin
        std::thread::sleep(Duration::from_micros(100));
    }
}

/// Aeron fragment handler - TRANSPORT LAYER ONLY
///
/// This handler only deals with:
/// 1. Parse correlation_id from incoming buffer
/// 2. Delegate to UbsCoreHandler for business logic
/// 3. Send response with correlation_id prepended
#[cfg(feature = "aeron")]
struct OrderHandler<'a> {
    handler: UbsCoreHandler<'a>,
    publication: &'a AeronPublication,
}

#[cfg(feature = "aeron")]
impl<'a> AeronFragmentHandlerCallback for OrderHandler<'a> {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        // 1. Extract correlation_id (transport concern)
        let (correlation_id, order_payload) = match parse_request(buffer) {
            Some(r) => r,
            None => {
                warn!("Message too short: {} bytes", buffer.len());
                return;
            }
        };

        // 2. Delegate to business logic (returns response bytes)
        let response_bytes = self.handler.on_message(order_payload);

        if response_bytes.is_empty() {
            return; // Invalid request, no response
        }

        // 3. Send response with correlation_id (transport concern)
        let mut message = Vec::with_capacity(8 + response_bytes.len());
        message.extend_from_slice(&correlation_id.to_le_bytes());
        message.extend_from_slice(&response_bytes);

        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let _ = self.publication.offer(&message, handler);
    }
}

