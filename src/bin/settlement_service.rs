use fetcher::configure;
use zmq::{Context, SUB};

fn main() {
    // 1. Load Config
    let config = configure::load_config().expect("Failed to load config");
    let zmq_config = config.zeromq.expect("ZMQ config missing");

    // 2. Setup ZMQ Subscriber
    let context = Context::new();
    let subscriber = context.socket(SUB).expect("Failed to create SUB socket");
    
    let endpoint = format!("tcp://localhost:{}", zmq_config.settlement_port);
    subscriber.connect(&endpoint).expect("Failed to connect to settlement port");
    subscriber.set_subscribe(b"").expect("Failed to subscribe");

    println!("Settlement Service started.");
    println!("Listening on {}", endpoint);

    // 3. Event Loop
    println!("Waiting for trades...");
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
        let topic = subscriber.recv_string(0).unwrap().unwrap();
        let data = subscriber.recv_bytes(0).unwrap();
        
        // println!("Received [{}]: {} bytes", topic, data.len());
        
        // Deserialize into MatchExecData
        match serde_json::from_slice::<fetcher::ledger::MatchExecData>(&data) {
            Ok(trade) => {
                let seq = trade.output_sequence;
                
                if next_sequence == 0 {
                    // First message, initialize sequence
                    next_sequence = seq + 1;
                } else if seq != next_sequence {
                    eprintln!("CRITICAL: GAP DETECTED! Expected: {}, Got: {}", next_sequence, seq);
                    // In a real system, we would request replay here.
                    // For now, just update to current to continue.
                    next_sequence = seq + 1;
                } else {
                    next_sequence += 1;
                }

                println!(
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
                    eprintln!("[Settlement] Failed to write to CSV: {}", e);
                }
                wtr.flush().unwrap();
            }
            Err(e) => {
                eprintln!("[Settlement] Failed to deserialize: {}", e);
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) {
                     println!("  Raw Data: {}", json);
                }
            }
        }
    }
}
