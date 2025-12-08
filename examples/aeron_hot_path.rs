//! Hot Path Implementation: Aeron IPC for ~100ns Latency
//!
//! This demonstrates the "Fast Path" from Matching Engine to Risk Engine
//! using shared memory (Aeron IPC) instead of network sockets.
//!
//! Key points:
//! - `aeron:ipc` uses RAM (mmap file in /dev/shm), not network
//! - `publication.offer()` is non-blocking, returns immediately
//! - `subscription.poll()` checks CPU cache line, zero wait
//! - Latency: ~100 nanoseconds (vs 150-500µs for ZeroMQ)

use rusteron_client::*;
use rusteron_media_driver::{AeronDriver, AeronDriverContext};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// The "Address" for Shared Memory
// "aeron:ipc" tells Aeron to use RAM, not UDP.
const CHANNEL: &str = "aeron:ipc";
const STREAM_ID: i32 = 1001;

/// Message sent from ME to Risk Engine via Hot Path
#[derive(Debug, Clone)]
#[repr(C)] // For predictable memory layout
struct FastFillEvent {
    event_id: u64,
    user_id: u64,
    asset_id: u32,
    amount: i64, // Positive = credit, Negative = debit
    timestamp_ns: u64,
}

impl FastFillEvent {
    fn to_bytes(&self) -> Vec<u8> {
        // In production: Use SBE (Simple Binary Encoding) for zero-copy
        // For demo: Simple bincode serialization
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
            .to_vec()
        }
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != std::mem::size_of::<Self>() {
            return None;
        }
        unsafe {
            let ptr = bytes.as_ptr() as *const Self;
            Some(ptr.read())
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Aeron IPC Hot Path Demo ===\n");

    // ---------------------------------------------------------
    // 0. SYSTEM START: Launch the Media Driver
    // In PROD, this runs as a separate C++ process (aeronmd).
    // Here we embed it for simplicity.
    // ---------------------------------------------------------
    let media_driver_ctx = AeronDriverContext::new()?;
    let (_stop_driver, _driver_handle) =
        AeronDriver::launch_embedded(media_driver_ctx.clone(), false);
    println!("[System] Media Driver Started");
    println!("[System] Shared Memory Location: /dev/shm/aeron-*\n");

    let running = Arc::new(AtomicBool::new(true));
    let running_risk = running.clone();
    let running_me = running.clone();

    // ---------------------------------------------------------
    // 1. RISK ENGINE (Subscriber)
    // Listens for "Dirty" Balance Updates via Shared Memory
    // ---------------------------------------------------------
    let risk_thread = thread::spawn(move || {
        let ctx = AeronContext::new().unwrap();
        let aeron = Aeron::new(&ctx).unwrap();
        aeron.start().unwrap();

        // Subscribe to the IPC Channel
        let channel_cstr = CString::new(CHANNEL).unwrap();
        let subscription = aeron
            .add_subscription(
                &channel_cstr,
                STREAM_ID,
                Handlers::no_available_image_handler(),
                Handlers::no_unavailable_image_handler(),
            )
            .unwrap();

        println!("[Risk Engine] Subscribed to {}", CHANNEL);
        println!("[Risk Engine] Waiting for Hot Path signals...\n");

        // Simulated balance state
        let mut speculative_balance: i64 = 0;
        let mut confirmed_balance: i64 = 10_000; // Start with 10,000 USDT

        // The handler that runs on every message
        let handler = move |buffer: &[u8], _header: AeronHeader| {
            let receive_time = Instant::now();

            if let Some(event) = FastFillEvent::from_bytes(buffer) {
                // Calculate latency (approximate - same machine)
                let send_time_ns = event.timestamp_ns;
                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;
                let latency_ns = now_ns.saturating_sub(send_time_ns);

                // Apply speculative credit
                speculative_balance += event.amount;

                println!("[Risk Engine] HOT PATH RECEIVED (latency: ~{}ns)", latency_ns);
                println!(
                    "              event_id={}, user={}, asset={}, amount={}",
                    event.event_id, event.user_id, event.asset_id, event.amount
                );
                println!(
                    "              Balance: confirmed={}, speculative={}, tradeable={}",
                    confirmed_balance,
                    speculative_balance,
                    confirmed_balance + speculative_balance
                );
                println!(
                    "              ACTION: User can trade with {} immediately!\n",
                    confirmed_balance + speculative_balance
                );
            }
        };

        // Wrap handler for C-interop
        let (mut closure, _holder) = Handler::leak_with_fragment_assembler(handler).unwrap();

        // HOT LOOP: Busy-spin polling (LMAX Disruptor style)
        while running_risk.load(Ordering::Relaxed) {
            // poll() is NON-BLOCKING
            // Returns 0 if no data, >0 if messages processed
            let _ = subscription.poll(Some(&mut closure), 10);

            // In PRODUCTION: Do NOT sleep. Busy spin for lowest latency.
            // thread::yield_now(); // Optional: yield to other threads
        }

        println!("[Risk Engine] Shutting down...");
    });

    // ---------------------------------------------------------
    // 2. MATCHING ENGINE (Publisher)
    // Sends "Fill" events via Shared Memory
    // ---------------------------------------------------------
    let me_thread = thread::spawn(move || {
        // Wait for Risk Engine to connect
        thread::sleep(Duration::from_millis(500));

        let ctx = AeronContext::new().unwrap();
        let aeron = Aeron::new(&ctx).unwrap();
        aeron.start().unwrap();

        let channel_cstr = CString::new(CHANNEL).unwrap();
        let publication = aeron.add_publication(&channel_cstr, STREAM_ID).unwrap();

        println!("[Matching Engine] Publisher ready on {}\n", CHANNEL);

        let mut event_id = 0u64;

        // Simulate trades
        for i in 0..5 {
            event_id += 1;

            let event = FastFillEvent {
                event_id,
                user_id: 1001,
                asset_id: 1,  // USDT
                amount: 5000, // Credit 5000 USDT
                timestamp_ns: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            };

            let buffer = event.to_bytes();

            // OFFER: Non-blocking write to ring buffer
            loop {
                let result = publication
                    .offer(&buffer, Handlers::no_reserved_value_supplier_handler())
                    .unwrap();

                if result > 0 {
                    println!("[Matching Engine] Trade #{}: Sold BTC for 5000 USDT", event_id);
                    println!("                  Sent {} bytes to Shared Memory\n", result);
                    break;
                } else if result == -1 {
                    // Not connected yet
                    thread::sleep(Duration::from_millis(10));
                } else {
                    // Backpressure - buffer full
                    println!("[Matching Engine] Backpressure! Risk Engine is slow.");
                    thread::yield_now();
                }
            }

            // Simulate 1 trade per second
            thread::sleep(Duration::from_secs(1));
        }

        println!("[Matching Engine] Demo complete. Shutting down...");
        running_me.store(false, Ordering::Relaxed);
    });

    me_thread.join().unwrap();
    thread::sleep(Duration::from_millis(100)); // Let risk engine process last message
    running.store(false, Ordering::Relaxed);
    risk_thread.join().unwrap();

    println!("\n=== Demo Complete ===");
    Ok(())
}

// ----------------------------------------------------------------------------
// PRODUCTION NOTES
// ----------------------------------------------------------------------------
//
// 1. SIMPLE BINARY ENCODING (SBE)
//    - Use SBE for zero-copy message encoding
//    - Define schemas in XML, generate Rust code
//    - Latency: essentially zero (just pointer math)
//
// 2. MEDIA DRIVER
//    - Run as standalone C++ process: `aeronmd`
//    - Configure: /dev/shm location, buffer sizes, threading mode
//    - DEDICATED threading mode for lowest latency
//
// 3. POLLING STRATEGY
//    - BUSY SPIN: Lowest latency, highest CPU usage
//    - BACK OFF: Sleep when idle, higher latency
//    - YIELDING: Yield to OS, middle ground
//
// 4. ERROR HANDLING
//    - publication.offer() can return:
//      -1: NOT_CONNECTED (wait and retry)
//      -2: BACK_PRESSURED (subscriber slow)
//      -3: ADMIN_ACTION (driver doing maintenance)
//    - Handle each appropriately
//
// 5. MONITORING
//    - Aeron provides counters for:
//      - Publications/Subscriptions active
//      - Bytes sent/received
//      - NAKs, retransmissions
//      - Backpressure events
//
// 6. SHARED MEMORY LOCATION
//    - Linux: /dev/shm/aeron-<user>
//    - macOS: /tmp/aeron-<user> (tmpfs)
//    - Configure via MediaDriverContext
