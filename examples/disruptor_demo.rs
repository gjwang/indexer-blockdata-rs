use disruptor::{BusySpin, Producer, Sequence};
use std::thread;
use std::time::Duration;

// 1. Define the Event (Data to be transferred)
#[derive(Debug)]
struct MarketEvent {
    id: u64,
    price: f64,
    symbol: String,
}

// Factory to initialize the Ring Buffer with empty events
fn event_factory() -> MarketEvent {
    MarketEvent {
        id: 0,
        price: 0.0,
        symbol: String::new(),
    }
}

fn main() {
    println!(">>> Starting Disruptor Demo");

    // 2. Build the Disruptor
    // Ring Buffer size must be a power of 2 (e.g., 1024, 4096)
    let buffer_size = 4096;

    let disruptor = disruptor::build_single_producer(buffer_size, event_factory, BusySpin)
        .handle_events_with(on_event) // Consumer logic
        .build();

    // 3. Get a Producer (The Disruptor struct itself acts as a producer in single-producer mode, or we clone it?)
    // Actually, let's try to use `disruptor` directly as producer if possible, or check docs via error.
    // But usually:
    let mut producer = disruptor;

    // 4. Spawn a thread to publish events (Producer)
    let handle = thread::spawn(move || {
        let symbols = ["BTC", "ETH", "SOL", "ADA", "XRP"];
        for i in 0..100 {
            let symbol = symbols[i % symbols.len()];
            let price = 1000.0 + (i as f64) * 0.5;

            // Publish into the Ring Buffer
            // The closure gives us mutable access to the pre-allocated event in the buffer
            producer.publish(|e| {
                e.id = i as u64;
                e.price = price;
                e.symbol.clear();
                e.symbol.push_str(symbol);
            });

            // Simulate some bursty input
            if i % 10 == 0 {
                thread::sleep(Duration::from_millis(1));
            }
        }
        println!(">>> Producer Finished Publishing 100 events");
    });

    // 5. Wait for producer to finish (Disruptor is processing in background threads)
    handle.join().unwrap();

    // Allow some time for consumer to drain
    thread::sleep(Duration::from_secs(1));

    // Disruptor is dropped here, shutting down the consumer thread
    println!(">>> Demo Completed");
}

// Consumer / Event Handler
fn on_event(event: &MarketEvent, sequence: Sequence, end_of_batch: bool) {
    // Simulate processing logic
    // In a real matching engine, this is where matching logic happens
    if event.id % 10 == 0 {
         println!(
            "[Consumer] Seq: {}, BatchEnd: {}, Data: {:?}",
            sequence, end_of_batch, event
        );
    }
}
