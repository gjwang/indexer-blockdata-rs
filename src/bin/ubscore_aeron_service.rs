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
    AeronConfig, UbsCoreHandler, parse_request,
};

#[cfg(feature = "aeron")]
use rusteron_client::*;

// Logging
const TARGET: &str = "UBSC";
macro_rules! info  { ($($arg:tt)*) => { log::info!(target: TARGET, $($arg)*) } }
macro_rules! warn  { ($($arg:tt)*) => { log::warn!(target: TARGET, $($arg)*) } }
macro_rules! error { ($($arg:tt)*) => { log::error!(target: TARGET, $($arg)*) } }

// Tuning constants
const POLL_LIMIT: usize = 10;            // Max fragments per poll
const HEARTBEAT_INTERVAL_SECS: u64 = 10; // Heartbeat log interval
const POLL_SLEEP_US: u64 = 100;          // Sleep between polls to avoid busy-spin

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
    // Batch buffer for collecting messages during poll
    let batch_buffer: std::cell::RefCell<Vec<(u64, Vec<u8>)>> = std::cell::RefCell::new(Vec::with_capacity(POLL_LIMIT));

    // Business state
    let business_state = std::cell::RefCell::new(BusinessState {
        ubs_core,
        wal,
        kafka_producer,
    });

    // Collector callback - just stores messages, doesn't process
    let collector = BatchCollector {
        batch: &batch_buffer,
    };
    let collector_wrapped = Handler::leak(collector);

    let mut count = 0u64;
    let mut last_log = Instant::now();

    loop {
        // Phase 1: Collect messages into batch
        if let Ok(fragments) = subscription.poll(Some(&collector_wrapped), POLL_LIMIT) {
            count += fragments as u64;
        }

        // Phase 2: Process batch with single flush
        {
            let mut batch = batch_buffer.borrow_mut();
            if !batch.is_empty() {
                let responses = business_state.borrow_mut().process_batch(&batch);

                // Phase 3: Send all responses
                for (correlation_id, response_bytes) in responses {
                    if !response_bytes.is_empty() {
                        let mut message = Vec::with_capacity(8 + response_bytes.len());
                        message.extend_from_slice(&correlation_id.to_le_bytes());
                        message.extend_from_slice(&response_bytes);

                        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
                        let _ = publication.offer(&message, handler);
                    }
                }

                batch.clear();
            }
        }

        // Log heartbeat
        if last_log.elapsed() > Duration::from_secs(HEARTBEAT_INTERVAL_SECS) {
            info!("[HEARTBEAT] processed={} messages", count);
            last_log = Instant::now();
        }

        // Small sleep to avoid busy-spin
        std::thread::sleep(Duration::from_micros(POLL_SLEEP_US));
    }
}

/// Business state - PURE business logic
#[cfg(feature = "aeron")]
struct BusinessState {
    ubs_core: UBSCore<SpotRiskModel>,
    wal: MmapWal,
    kafka_producer: Option<FutureProducer>,
}

#[cfg(feature = "aeron")]
impl BusinessState {
    fn process_batch(&mut self, items: &[(u64, Vec<u8>)]) -> Vec<(u64, Vec<u8>)> {
        let mut handler = UbsCoreHandler {
            ubs_core: &mut self.ubs_core,
            wal: &mut self.wal,
            kafka_producer: &self.kafka_producer,
        };
        handler.process_batch(items)
    }
}

/// Batch collector - just stores messages during poll, doesn't process
#[cfg(feature = "aeron")]
struct BatchCollector<'a> {
    batch: &'a std::cell::RefCell<Vec<(u64, Vec<u8>)>>,
}

#[cfg(feature = "aeron")]
impl<'a> AeronFragmentHandlerCallback for BatchCollector<'a> {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        let (correlation_id, payload) = match parse_request(buffer) {
            Some(r) => r,
            None => {
                warn!("Message too short: {} bytes", buffer.len());
                return;
            }
        };

        // Just collect, don't process
        self.batch.borrow_mut().push((correlation_id, payload.to_vec()));
    }
}
