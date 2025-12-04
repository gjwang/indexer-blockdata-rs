use fetcher::configure;
use fetcher::logger::setup_logger_for_service;
use zmq::{Context, SUB};
use log::{info, error, debug};

fn main() {
    // Setup service-specific logger (logs to log/settlement.log)
    if let Err(e) = setup_logger_for_service(Some("settlement")) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    // 2. Load Config
    let config = configure::load_config().expect("Failed to load config");
    let zmq_config = config.zeromq.expect("ZMQ config missing");

    // 3. Setup ZMQ Subscriber
    let context = Context::new();
    let subscriber = context.socket(SUB).expect("Failed to create SUB socket");
    
    let endpoint = format!("tcp://localhost:{}", zmq_config.settlement_port);
    subscriber.connect(&endpoint).expect("Failed to connect to settlement port");
    subscriber.set_subscribe(b"").expect("Failed to subscribe");

    info!("Settlement Service started.");
    info!("Listening on {}", endpoint);

    // 4. Event Loop
    info!("Waiting for trades...");
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
        let topic = match subscriber.recv_string(0) {
            Ok(Ok(t)) => t,
            Ok(Err(_)) => {
                error!("Failed to decode topic string");
                continue;
            }
            Err(e) => {
                error!("ZMQ recv error: {}", e);
                continue;
            }
        };
        
        let data = match subscriber.recv_bytes(0) {
            Ok(d) => d,
            Err(e) => {
                error!("ZMQ recv bytes error: {}", e);
                continue;
            }
        };
        
        // println!("Received [{}]: {} bytes", topic, data.len());
        
        // Deserialize into MatchExecData
        match serde_json::from_slice::<fetcher::ledger::MatchExecData>(&data) {
            Ok(trade) => {
                let seq = trade.output_sequence;
                
                if next_sequence == 0 {
                    // First message, initialize sequence
                    next_sequence = seq + 1;
                } else if seq != next_sequence {
                    error!("CRITICAL: GAP DETECTED! Expected: {}, Got: {}", next_sequence, seq);
                    // In a real system, we would request replay here.
                    // For now, just update to current to continue.
                    next_sequence = seq + 1;
                } else {
                    next_sequence += 1;
                }

                info!(
                    "[Settlement] Seq: {}, TradeID: {}, Price: {}, Qty: {}, Buyer: {}, Seller: {}",
                    trade.output_sequence,
                    trade.trade_id,
                    trade.price,
                    trade.quantity,
                    trade.buyer_user_id,
                    trade.seller_user_id
                );
                
                // Persist to CSV
                if let Err(e) = wtr.serialize(&trade) {
                    error!("[Settlement] Failed to write to CSV: {}", e);
                }
                if let Err(e) = wtr.flush() {
                    error!("[Settlement] Failed to flush CSV: {}", e);
                }
            }
            Err(e) => {
                error!("[Settlement] Failed to deserialize: {}", e);
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) {
                     debug!("  Raw Data: {}", json);
                }
            }
        }
    }
}
