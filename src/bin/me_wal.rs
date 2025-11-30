use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use memmap2::MmapMut;
use nix::unistd::{fork, ForkResult};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// Only use Jemalloc on Linux/Mac (Unix), not Windows
#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

// Use the shared library module
use fetcher::order_wal::{Wal, LogEntry, WalSide};

// =================================================================
// MEMORY ALLOCATOR CONFIG
// =================================================================

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

// ==========================================
// REDIS-STYLE MATCHING ENGINE
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: u64,
    pub symbol: [u8; 8],
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
    pub user_id: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Trade {
    pub match_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    pub last_seq: u64,
    pub match_sequence: u64,
    pub orders: FxHashMap<u64, Order>,
    pub trade_history: Vec<Trade>,
}

pub struct MatchingEngine {
    pub orders: FxHashMap<u64, Order>,
    pub match_sequence: u64,
    pub trade_history: Vec<Trade>,
    pub wal: Wal,
    pub snapshot_dir: PathBuf,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        fs::create_dir_all(snapshot_dir)?;
        let orders = FxHashMap::default();
        let wal = Wal::open(wal_path, 0)?;
        Ok(Self { 
            orders, 
            match_sequence: 0,
            trade_history: Vec::new(),
            wal, 
            snapshot_dir: snapshot_dir.to_path_buf() 
        })
    }

    #[inline(always)]
    pub fn place_order(&mut self, order: Order) {
        let wal_side = match order.side {
            OrderSide::Buy => WalSide::Buy,
            OrderSide::Sell => WalSide::Sell,
        };

        let symbol_str = std::str::from_utf8(&order.symbol).unwrap_or("UNKNOWN").trim_matches('\0').to_string();

        self.wal.append(LogEntry::PlaceOrder {
            order_id: order.id,
            symbol: symbol_str,
            side: wal_side,
            price: order.price,
            quantity: order.quantity,
            user_id: order.user_id,
            timestamp: order.timestamp,
        });
        self.orders.insert(order.id, order);
    }

    /// Simulate matching two orders and create a trade
    pub fn match_orders(&mut self, buy_order_id: u64, sell_order_id: u64, price: u64, quantity: u64) -> Option<Trade> {
        // Verify both orders exist
        if !self.orders.contains_key(&buy_order_id) || !self.orders.contains_key(&sell_order_id) {
            return None;
        }

        // Increment match sequence and create trade
        self.match_sequence += 1;
        let trade = Trade {
            match_id: self.match_sequence,
            buy_order_id,
            sell_order_id,
            price,
            quantity,
        };

        // Log trade to WAL
        self.wal.append(LogEntry::Trade {
            match_id: trade.match_id,
            buy_order_id: trade.buy_order_id,
            sell_order_id: trade.sell_order_id,
            price: trade.price,
            quantity: trade.quantity,
        });
        
        // Add to trade history
        self.trade_history.push(trade);

        println!("Trade Executed: match_id={}, buy={}, sell={}, price={}, qty={}", 
            trade.match_id, buy_order_id, sell_order_id, price, quantity);

        Some(trade)
    }

    pub fn cancel_order(&mut self, order_id: u64) -> bool {
        if self.orders.remove(&order_id).is_some() {
            self.wal.append(LogEntry::CancelOrder { id: order_id });
            true
        } else {
            false
        }
    }


    // =========================================================
    // THE "REDIS" COPY-ON-WRITE SNAPSHOT
    // =========================================================
    pub fn trigger_cow_snapshot(&self) {
        let current_seq = self.wal.current_seq;
        let snap_dir = self.snapshot_dir.clone();

        // flush stdout so logs don't get duplicated in child
        let _ = std::io::stdout().flush();

        // 1. FORK THE PROCESS
        match unsafe { fork() } {
            Ok(ForkResult::Parent { child: _ }) => {
                // --- PARENT PROCESS ---
            }
            Ok(ForkResult::Child) => {
                // --- CHILD PROCESS ---
                let start = Instant::now();
                let filename = format!("snapshot_{}.snap", current_seq);
                let path = snap_dir.join(filename);

                let snap = Snapshot {
                    last_seq: current_seq,
                    match_sequence: self.match_sequence,
                    orders: self.orders.clone(),
                    trade_history: self.trade_history.clone(),
                };

                // Perform the slow Write
                if let Ok(file) = File::create(&path) {
                    let writer = BufWriter::new(file);
                    if let Err(e) = bincode::serialize_into(writer, &snap) {
                        eprintln!("Child failed to write: {:?}", e);
                    }
                }

                println!(
                    "   [Child PID {}] Snapshot {} Saved. Time: {:.2?}",
                    std::process::id(), current_seq, start.elapsed()
                );

                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("Fork failed: {}", e);
            }
        }
    }
}

// ==========================================
// MAIN
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("cow.wal");
    let snap_dir = Path::new("cow_snaps");
    if wal_path.exists() { fs::remove_file(wal_path)?; }
    if snap_dir.exists() { fs::remove_dir_all(snap_dir)?; }

    let total = 1_000_000;
    println!(">>> STARTING MATCHING ENGINE WITH TRADE TRACKING ({} Orders)", total);

    let mut engine = MatchingEngine::new(wal_path, snap_dir)?;
    let start = Instant::now();
    let symbol = *b"BTC_USDT";

    // Store some order IDs for matching demonstration
    let mut buy_orders = Vec::new();
    let mut sell_orders = Vec::new();

    for i in 1..=total {
        let side = if i % 2 == 0 { OrderSide::Buy } else { OrderSide::Sell };
        // Use u64 ID (simulated by i)
        let order_id = i as u64;
        
        let order = Order {
            id: order_id,
            symbol,
            side,
            price: 50000,
            quantity: 1,
            user_id: 1000 + i,
            timestamp: 0,
        };

        // Store order IDs for matching
        if side == OrderSide::Buy {
            buy_orders.push(order.id);
        } else {
            sell_orders.push(order.id);
        }

        engine.place_order(order);

        // Match orders every 100 orders to demonstrate trade creation
        if i % 100 == 0 && !buy_orders.is_empty() && !sell_orders.is_empty() {
            let buy_id = buy_orders.pop().unwrap();
            let sell_id = sell_orders.pop().unwrap();
            engine.match_orders(buy_id, sell_id, 50000, 1);
        }

        // Cancel an order every 500 orders
        if i % 500 == 0 {
            if let Some(id) = buy_orders.pop() {
                if engine.cancel_order(id) {
                    println!("    Cancelled Buy Order {}", id);
                }
            }
        }

        // Snapshot every 200k
        if i % 200_000 == 0 {
            let t = Instant::now();
            engine.trigger_cow_snapshot();
            // This print proves the Main Thread barely paused
            println!("    Forked at Order {}. Main Thread Paused: {:.2?}", i, t.elapsed());
        }
    }

    let dur = start.elapsed();
    println!("\n>>> DONE");
    println!("    Total Orders: {}", total);
    println!("    Total Trades: {}", engine.trade_history.len());
    println!("    Last Match ID: {}", engine.match_sequence);
    println!("    Total Time: {:.2?}", dur);
    println!("    Throughput: {:.0} orders/sec", total as f64 / dur.as_secs_f64());

    // Wait for children to finish (for demo purposes)
    thread::sleep(Duration::from_secs(3));
    Ok(())
}