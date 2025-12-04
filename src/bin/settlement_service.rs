use fetcher::configure;
use fetcher::logger::setup_logger;
use zmq::{Context, SUB};

// Use custom log macros with target "settlement" for cleaner logs
const LOG_TARGET: &str = "settlement";

fn main() {
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

    // Setup ZMQ Subscriber
    let context = Context::new();
    let subscriber = context.socket(SUB).expect("Failed to create SUB socket");
    
    let endpoint = format!("tcp://localhost:{}", zmq_config.settlement_port);
    subscriber.connect(&endpoint).expect("Failed to connect to settlement port");
    subscriber.set_subscribe(b"").expect("Failed to subscribe");

    // Print boot parameters
    log::info!(target: LOG_TARGET, "=== Settlement Service Boot Parameters ===");
    log::info!(target: LOG_TARGET, "  ZMQ Endpoint:     {}", endpoint);
    log::info!(target: LOG_TARGET, "  Log File:         {}", config.log_file);
    log::info!(target: LOG_TARGET, "  Log Level:        {}", config.log_level);
    log::info!(target: LOG_TARGET, "  Log to File:      {}", config.log_to_file);
    log::info!(target: LOG_TARGET, "===========================================");

    log::info!(target: LOG_TARGET, "Settlement Service started.");
    log::info!(target: LOG_TARGET, "Listening on {}", endpoint);

    // Event Loop
    log::info!(target: LOG_TARGET, "Waiting for trades...");
    let mut next_sequence: u64 = 0;
    
    // Open CSV Writer in Append Mode
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open("settled_trades.csv")
        .expect("Failed to open settled_trades.csv");
        
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false) // Don't write headers on append (unless file is empty, but let's keep it simple)
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
                
                if next_sequence == 0 {
                    // First message, initialize sequence
                    next_sequence = seq + 1;
                } else if seq != next_sequence {
                    log::error!(target: LOG_TARGET, "CRITICAL: GAP DETECTED! Expected: {}, Got: {}", next_sequence, seq);
                    // In a real system, we would request replay here.
                    // For now, just update to current to continue.
                    next_sequence = seq + 1;
                } else {
                    next_sequence += 1;
                }

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
                
                // Persist to CSV
                if let Err(e) = wtr.serialize(&trade) {
                    log::error!(target: LOG_TARGET, "Failed to write to CSV: {}", e);
                }
                if let Err(e) = wtr.flush() {
                    log::error!(target: LOG_TARGET, "Failed to flush CSV: {}", e);
                }
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
