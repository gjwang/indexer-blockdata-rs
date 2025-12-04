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
    loop {
        let topic = subscriber.recv_string(0).unwrap().unwrap();
        let data = subscriber.recv_bytes(0).unwrap();
        
        println!("Received [{}]: {} bytes", topic, data.len());
        
        // TODO: Deserialize and process
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) {
             println!("  Data: {}", json);
        }
    }
}
