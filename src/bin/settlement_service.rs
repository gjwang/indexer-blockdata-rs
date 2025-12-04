use fetcher::configure;
use fetcher::db::SettlementDb;
use fetcher::logger::setup_logger;
use zmq::{Context, SUB};

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
            db
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to ScyllaDB: {}", e);
            log::error!(target: LOG_TARGET, "   Make sure ScyllaDB is running: docker-compose up -d scylla");
            log::error!(target: LOG_TARGET, "   And schema is initialized: ./scripts/init_scylla.sh");
            return;
        }
    };

    // Health check
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

    // Setup ZMQ Subscriber
    let context = Context::new();
    let subscriber = context.socket(SUB).expect("Failed to create SUB socket");
    
    let endpoint = format!("tcp://localhost:{}", zmq_config.settlement_port);
    subscriber.connect(&endpoint).expect("Failed to connect to settlement port");
    subscriber.set_subscribe(b"").expect("Failed to subscribe");

    // Print boot parameters
    log::info!(target: LOG_TARGET, "=== Settlement Service Boot Parameters ===");
    log::info!(target: LOG_TARGET, "  ZMQ Endpoint:     {}", endpoint);
    log::info!(target: LOG_TARGET, "  ScyllaDB Hosts:   {:?}", scylla_config.hosts);
    log::info!(target: LOG_TARGET, "  ScyllaDB Keyspace: {}", scylla_config.keyspace);
    log::info!(target: LOG_TARGET, "  Log File:         {}", config.log_file);
    log::info!(target: LOG_TARGET, "  Log Level:        {}", config.log_level);
    log::info!(target: LOG_TARGET, "  Log to File:      {}", config.log_to_file);
    log::info!(target: LOG_TARGET, "===========================================");

    log::info!(target: LOG_TARGET, "Settlement Service started.");
    log::info!(target: LOG_TARGET, "Listening on {}", endpoint);

    // Event Loop
    log::info!(target: LOG_TARGET, "Waiting for trades...");
    let mut next_sequence: u64 = 0;
    let mut total_settled: u64 = 0;
    
    // Open CSV Writer in Append Mode (keep as backup/audit trail)
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open("settled_trades.csv")
        .expect("Failed to open settled_trades.csv");
        
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
        
        // Deserialize into MatchExecData
        match serde_json::from_slice::<fetcher::ledger::MatchExecData>(&data) {
            Ok(trade) => {
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

                //TODO: add profiling here
                // Persist to ScyllaDB (primary storage)
                match settlement_db.insert_trade(&trade).await {
                    Ok(()) => {
                        total_settled += 1;
                        if total_settled % 100 == 0 {
                            log::info!(target: LOG_TARGET, "Total trades settled: {}", total_settled);
                        }
                    }
                    Err(e) => {
                        log::error!(target: LOG_TARGET, "Failed to insert trade to ScyllaDB: {}", e);
                        
                        // Log to failed_trades.json for manual recovery
                        let failed_file = std::fs::OpenOptions::new()
                            .write(true)
                            .create(true)
                            .append(true)
                            .open("failed_trades.json");
                            
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

                // Log trade details
                log::info!(
                    target: LOG_TARGET,
                    "Seq: {}, TradeID: {}, Price: {}, Qty: {}, Buyer: {}, Seller: {}",
                    trade.output_sequence,
                    trade.trade_id,
                    trade.price,
                    trade.quantity,
                    trade.buyer_user_id,
                    trade.seller_user_id
                );
            }
            Err(e) => {
                log::error!(target: LOG_TARGET, "Failed to deserialize: {}", e);
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) {
                     log::debug!(target: LOG_TARGET, "  Raw Data: {}", json);
                }
            }
        }
    }
}
