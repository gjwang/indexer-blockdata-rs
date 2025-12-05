use fetcher::configure;
use fetcher::db::SettlementDb;
use fetcher::logger::setup_logger;
use zmq::{Context, SUB};

use fetcher::starrocks_client::{StarRocksClient, StarRocksTrade};

// Use custom log macros with target "settlement" for cleaner logs
const LOG_TARGET: &str = "settlement";

#[tokio::main]
async fn main() {
    // Load service-specific configuration (config/settlement_config.yaml)
    // This isolates settlement config from other services
    let config = configure::load_service_config("settlement_config")
        .expect("Failed to load settlement configuration");

    // Setup logger using config (log file path comes from config/settlement.yaml)
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    // Get ZMQ configuration
    let zmq_config = config.zeromq.expect("ZMQ config missing");

    // Get ScyllaDB configuration
    let scylla_config = config.scylladb.expect("ScyllaDB config missing");

    // Initialize ScyllaDB connection
    log::info!(target: LOG_TARGET, "Connecting to ScyllaDB...");
    let settlement_db = match SettlementDb::connect(&scylla_config).await {
        Ok(db) => {
            log::info!(target: LOG_TARGET, "✅ Connected to ScyllaDB");
            std::sync::Arc::new(db)
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to ScyllaDB: {}", e);
            log::error!(target: LOG_TARGET, "   Make sure ScyllaDB is running: docker-compose up -d scylla");
            log::error!(target: LOG_TARGET, "   And schema is initialized: ./scripts/init_scylla.sh");
            return;
        }
    };

    // Initialize StarRocks Client
    let starrocks_client = std::sync::Arc::new(StarRocksClient::new());

    // Initial Health check
    match settlement_db.health_check().await {
        Ok(true) => log::info!(target: LOG_TARGET, "✅ ScyllaDB health check passed"),
        Ok(false) => {
            log::error!(target: LOG_TARGET, "❌ ScyllaDB health check failed");
            return;
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ ScyllaDB health check error: {}", e);
            return;
        }
    }

    // Spawn background health check task
    let db_clone = settlement_db.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            match db_clone.health_check().await {
                Ok(true) => log::debug!(target: LOG_TARGET, "[HEALTH] ScyllaDB connection active"),
                Ok(false) => log::error!(target: LOG_TARGET, "[HEALTH] ScyllaDB connection lost!"),
                Err(e) => {
                    log::error!(target: LOG_TARGET, "[HEALTH] ScyllaDB health check error: {}", e)
                }
            }
        }
    });

    // Setup ZMQ Subscriber
    let context = Context::new();
    let subscriber = context.socket(SUB).expect("Failed to create SUB socket");

    let endpoint = format!("tcp://localhost:{}", zmq_config.settlement_port);
    subscriber.connect(&endpoint).expect("Failed to connect to settlement port");
    subscriber.set_subscribe(b"").expect("Failed to subscribe");

    // Validate required file paths from config
    if config.backup_csv_file.is_empty() {
        log::error!(target: LOG_TARGET, "❌ backup_csv_file not configured in settlement_config.yaml");
        log::error!(target: LOG_TARGET, "   Please add: backup_csv_file: \"path/to/settled_trades.csv\"");
        return;
    }
    if config.failed_trades_file.is_empty() {
        log::error!(target: LOG_TARGET, "❌ failed_trades_file not configured in settlement_config.yaml");
        log::error!(target: LOG_TARGET, "   Please add: failed_trades_file: \"path/to/failed_trades.json\"");
        return;
    }

    // Prepare data directory (expand tilde and create if needed)
    let data_dir = match configure::prepare_data_dir(&config.data_dir) {
        Ok(dir) => dir,
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ {}", e);
            return;
        }
    };

    // Resolve file paths (relative to data_dir if not absolute)
    let backup_csv_file = if std::path::Path::new(&config.backup_csv_file).is_absolute() {
        config.backup_csv_file.clone()
    } else {
        format!("{}/{}", data_dir, config.backup_csv_file)
    };

    let failed_trades_file = if std::path::Path::new(&config.failed_trades_file).is_absolute() {
        config.failed_trades_file.clone()
    } else {
        format!("{}/{}", data_dir, config.failed_trades_file)
    };

    // Print boot parameters
    log::info!(target: LOG_TARGET, "=== Settlement Service Boot Parameters ===");
    log::info!(target: LOG_TARGET, "  ZMQ Endpoint:     {}", endpoint);
    log::info!(target: LOG_TARGET, "  ScyllaDB Hosts:   {:?}", scylla_config.hosts);
    log::info!(target: LOG_TARGET, "  ScyllaDB Keyspace: {}", scylla_config.keyspace);
    log::info!(target: LOG_TARGET, "  Log File:         {}", config.log_file);
    log::info!(target: LOG_TARGET, "  Log Level:        {}", config.log_level);
    log::info!(target: LOG_TARGET, "  Log to File:      {}", config.log_to_file);
    log::info!(target: LOG_TARGET, "  Data Directory:   {}", data_dir);
    log::info!(target: LOG_TARGET, "  Backup CSV:       {}", backup_csv_file);
    log::info!(target: LOG_TARGET, "  Failed Trades:    {}", failed_trades_file);
    log::info!(target: LOG_TARGET, "===========================================");

    log::info!(target: LOG_TARGET, "Settlement Service started.");
    log::info!(target: LOG_TARGET, "Listening on {}", endpoint);

    // Event Loop
    log::info!(target: LOG_TARGET, "Waiting for trades...");
    let mut next_sequence: u64 = 0;
    let mut total_settled: u64 = 0;
    let mut total_errors: u64 = 0;

    // Open CSV Writer in Append Mode (keep as backup/audit trail)
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(backup_csv_file)
        .expect("Failed to open backup CSV file");

    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false) // Don't write headers on append
        .from_writer(file);

    loop {
        let _topic = match subscriber.recv_string(0) {
            Ok(Ok(t)) => t,
            Ok(Err(_)) => {
                log::error!(target: LOG_TARGET, "Failed to decode topic string");
                continue;
            }
            Err(e) => {
                log::error!(target: LOG_TARGET, "ZMQ recv error: {}", e);
                continue;
            }
        };

        let data = match subscriber.recv_bytes(0) {
            Ok(d) => d,
            Err(e) => {
                log::error!(target: LOG_TARGET, "ZMQ recv bytes error: {}", e);
                continue;
            }
        };

        // Try MatchExecData (Trade)
        if let Ok(trade) = serde_json::from_slice::<fetcher::ledger::MatchExecData>(&data) {
            let seq = trade.output_sequence;

            // Sequence validation
            if next_sequence == 0 {
                // First message, initialize sequence
                next_sequence = seq + 1;
                log::info!(target: LOG_TARGET, "Initialized sequence tracking at {}", seq);
            } else if seq != next_sequence {
                log::error!(
                    target: LOG_TARGET,
                    "CRITICAL: GAP DETECTED! Expected: {}, Got: {}. Gap size: {}",
                    next_sequence,
                    seq,
                    seq as i64 - next_sequence as i64
                );
                // In production, we would request replay here
                next_sequence = seq + 1;
            } else {
                next_sequence += 1;
            }

            // Get symbol from asset IDs
            let symbol = match fetcher::symbol_utils::get_symbol_from_assets(
                trade.base_asset,
                trade.quote_asset,
            ) {
                Ok(s) => s,
                Err(e) => {
                    log::error!(target: LOG_TARGET, "Failed to get symbol for trade {}: {}", trade.trade_id, e);
                    continue;
                }
            };


            //TODO:  user optimzation settlement strategy: assume not settled, will fail on duplicate key
            // Check if already settled (idempotency)
            let already_settled = match settlement_db.trade_exists(&symbol, trade.trade_id).await {
                Ok(exists) => exists,
                Err(e) => {
                    log::error!(target: LOG_TARGET, "Failed to check if trade exists: {}", e);
                    false // Assume not settled, will fail on duplicate key
                }
            };

            if already_settled {
                log::debug!(target: LOG_TARGET, "Trade {} already settled, skipping", trade.trade_id);
                continue;
            }

            // Settle atomically using LOGGED BATCH
            match settlement_db.settle_trade_atomically(&symbol, &trade).await {
                Ok(()) => {
                    total_settled += 1;

                    // Async load to StarRocks (Analytics)
                    let sr_client = starrocks_client.clone();
                    let ts = if trade.settled_at > 0 {
                        trade.settled_at as i64
                    } else {
                        chrono::Utc::now().timestamp_millis()
                    };
                    let sr_trade = StarRocksTrade::from_match_exec(&trade, ts);

                    tokio::spawn(async move {
                        if let Err(e) = sr_client.load_trade(sr_trade).await {
                            log::error!(target: LOG_TARGET, "Failed to load trade to StarRocks: {}", e);
                        }
                    });

                    if total_settled % 100 == 0 {
                        log::info!(target: LOG_TARGET, "Total trades settled: {}", total_settled);
                        log::info!(target: LOG_TARGET, "[METRIC] settlement_trades_total={}", total_settled);
                    }

                    log::info!(
                        target: LOG_TARGET,
                        "✅ Settled trade {} on {} (seq={}, buyer={}, seller={}, price={}, qty={})",
                        trade.trade_id,
                        symbol,
                        trade.output_sequence,
                        trade.buyer_user_id,
                        trade.seller_user_id,
                        trade.price,
                        trade.quantity
                    );
                }
                Err(e) => {
                    total_errors += 1;
                    log::error!(target: LOG_TARGET, "❌ Failed to settle trade {}: {}", trade.trade_id, e);
                    log::error!(target: LOG_TARGET, "[METRIC] settlement_errors_total={}", total_errors);

                    // Log to failed_trades.json for manual recovery
                    let failed_file = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .append(true)
                        .open(&failed_trades_file);

                    match failed_file {
                        Ok(mut f) => {
                            if let Ok(json) = serde_json::to_string(&trade) {
                                use std::io::Write;
                                let _ = writeln!(f, "{}", json);
                            }
                        }
                        Err(io_err) => {
                            log::error!(target: LOG_TARGET, "Failed to write to failed_trades.json: {}", io_err);
                        }
                    }
                }
            }

            // Persist to CSV (backup/audit trail)
            if let Err(e) = wtr.serialize(&trade) {
                log::error!(target: LOG_TARGET, "Failed to write to CSV: {}", e);
            }
            if let Err(e) = wtr.flush() {
                log::error!(target: LOG_TARGET, "Failed to flush CSV: {}", e);
            }
        }
        // Try LedgerEvent (Deposit/Withdrawal)
        else if let Ok(mut event) = serde_json::from_slice::<fetcher::ledger::LedgerEvent>(&data)
        {
            // Assign a unique sequence ID (using timestamp nanos) since engine sent 0
            event.sequence_id = chrono::Utc::now().timestamp_nanos() as u64;

            match settlement_db.insert_ledger_event(&event).await {
                Ok(()) => {
                    log::info!(target: LOG_TARGET, "Settled Event: {} for User {}", event.event_type, event.user_id);

                    // Update user_balances for DEPOSIT events
                    if event.event_type == "DEPOSIT" {
                        // For deposits, we don't have a version from ME, so we use a simple increment
                        // This is safe because deposits happen at initialization before any trades
                        match settlement_db.update_balance_for_deposit(event.user_id, event.currency, event.amount).await {
                            Ok((current_available, new_available, current_version, new_version)) => {
                                log::info!(
                                    target: LOG_TARGET,
                                    "Balance updated for deposit: user={}, asset={}, {} -> {}, v={} -> {}",
                                    event.user_id,
                                    event.currency,
                                    current_available,
                                    new_available,
                                    current_version,
                                    new_version
                                );
                            }
                            Err(e) => {
                                log::error!(target: LOG_TARGET, "Failed to update balance for deposit: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    total_errors += 1;
                    log::error!(target: LOG_TARGET, "Failed to insert event to ScyllaDB: {}", e);
                    log::error!(target: LOG_TARGET, "[METRIC] settlement_db_errors_total={}", total_errors);
                }
            }
        } else {
            log::error!(target: LOG_TARGET, "Failed to deserialize message (unknown format)");
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) {
                log::debug!(target: LOG_TARGET, "  Raw Data: {}", json);
            }
        }
    }
}
