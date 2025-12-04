use zmq::{Context, SUB};

fn main() {
    let context = Context::new();
    let subscriber = context.socket(SUB).unwrap();
    
    // Connect to both ports
    subscriber.connect("tcp://localhost:5557").unwrap();
    subscriber.connect("tcp://localhost:5558").unwrap();
    
    // Subscribe to all
    subscriber.set_subscribe(b"").unwrap();
    
    println!("Listening on ZMQ ports 5557 (Settlement) and 5558 (Market Data)...");
    
    loop {
        let topic = subscriber.recv_string(0).unwrap().unwrap();
        let data = subscriber.recv_bytes(0).unwrap();
        
        println!("Received on [{}]: {} bytes", topic, data.len());
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data) {
            println!("  Data: {}", json);
        }
    }
}
