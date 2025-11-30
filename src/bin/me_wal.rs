use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use flatbuffers::FlatBufferBuilder;
use memmap2::MmapMut;
use nix::sys::wait::waitpid;
// NEW: Unix System Calls
use nix::unistd::{fork, ForkResult};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
// Only use Jemalloc on Linux/Mac (Unix), not Windows
#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;
use ulid::Ulid;

use wal_schema::wal_schema::{
    Cancel, CancelArgs, EntryType, Order as FbsOrder, OrderArgs,
    OrderSide as FbsSide, Trade as FbsTrade, TradeArgs, UlidStruct, WalFrame, WalFrameArgs,
};

// =================================================================
// MEMORY ALLOCATOR CONFIG
// =================================================================

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

// [Previous imports...]
#[allow(dead_code, unused_imports)]
#[path = "wal_generated.rs"]
// mod wal_schema;
#[allow(dead_code, unused_imports)]
mod wal_schema {
    include!(concat!(env!("OUT_DIR"), "/wal_generated.rs"));
}

// Needed for cleanup if strictly safe

// [Domain Types & Wal Struct - SAME AS BEFORE, Omitted for brevity]
// ... (Paste Order, OrderSide, Wal, Wal Implementation here) ...

// ==========================================
// REDIS-STYLE MATCHING ENGINE
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: Ulid,
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
    pub buy_order_id: Ulid,
    pub sell_order_id: Ulid,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid },
    Trade(Trade),
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    pub last_seq: u64,
    pub match_sequence: u64,
    pub orders: FxHashMap<Ulid, Order>,
    pub trade_history: Vec<Trade>,
}

fn to_fbs_ulid(u: Ulid) -> UlidStruct {
    let n = u.0;
    UlidStruct::new((n >> 64) as u64, n as u64)
}

fn from_fbs_ulid(f: &UlidStruct) -> Ulid {
    Ulid(((f.hi() as u128) << 64) | (f.lo() as u128))
}
// ==========================================
// 3. WAL (Same Auto-Growing Mmap)
// ==========================================

pub struct Wal {
    tx: Sender<LogEntry>,
    pub current_seq: u64,
}

impl Wal {
    pub fn open(path: &Path, start_seq: u64) -> Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = bounded(2_000_000);

        thread::spawn(move || {
            Self::background_writer(path, rx, start_seq);
        });

        Ok(Self { tx, current_seq: start_seq })
    }

    fn background_writer(path: PathBuf, rx: Receiver<LogEntry>, mut current_seq: u64) {
        let file = OpenOptions::new().read(true).write(true).create(true).open(&path).unwrap();
        let mut file_len = 1024 * 1024 * 1024;
        if file.metadata().unwrap().len() < file_len { file.set_len(file_len).unwrap(); }
        let mut mmap_opt = Some(unsafe { MmapMut::map_mut(&file).unwrap() });
        let mut builder = FlatBufferBuilder::new();

        let mut cursor = 0;
        {
            let mmap = mmap_opt.as_ref().unwrap();
            while cursor + 4 < file_len as usize {
                let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
                let len = u32::from_le_bytes(len_bytes) as usize;
                if len == 0 { break; }
                cursor += 4 + len;
            }
        }

        const SYNC_BATCH: usize = 5000;
        let mut unsynced_writes = 0;

        loop {
            match rx.recv() {
                Ok(entry) => {
                    current_seq += 1;
                    builder.reset();

                    let (entry_type, entry_offset) = match entry {
                        LogEntry::PlaceOrder(o) => {
                            let s_str = std::str::from_utf8(&o.symbol).unwrap_or("UNKNOWN").trim_matches('\0');
                            let f_sym = builder.create_string(s_str);
                            let f_side = match o.side {
                                OrderSide::Buy => FbsSide::Buy,
                                OrderSide::Sell => FbsSide::Sell
                            };
                            let f_ulid = to_fbs_ulid(o.id);
                            let order = FbsOrder::create(&mut builder, &OrderArgs {
                                id: Some(&f_ulid),
                                symbol: Some(f_sym),
                                side: f_side,
                                price: o.price,
                                quantity: o.quantity,
                                user_id: o.user_id,
                                timestamp: o.timestamp,
                            });
                            (EntryType::Order, order.as_union_value())
                        }
                        LogEntry::CancelOrder { id } => {
                            let f_ulid = to_fbs_ulid(id);
                            let cancel = Cancel::create(&mut builder, &CancelArgs { id: Some(&f_ulid) });
                            (EntryType::Cancel, cancel.as_union_value())
                        }
                        LogEntry::Trade(t) => {
                            let buy_id = to_fbs_ulid(t.buy_order_id);
                            let sell_id = to_fbs_ulid(t.sell_order_id);
                            let trade = FbsTrade::create(&mut builder, &TradeArgs {
                                match_id: t.match_id,
                                buy_order_id: Some(&buy_id),
                                sell_order_id: Some(&sell_id),
                                price: t.price,
                                quantity: t.quantity,
                            });
                            (EntryType::Trade, trade.as_union_value())
                        }
                    };

                    let frame = WalFrame::create(&mut builder, &WalFrameArgs {
                        seq: current_seq,
                        entry_type,
                        entry: Some(entry_offset),
                    });

                    builder.finish(frame, None);
                    let buf = builder.finished_data();
                    let size = buf.len();

                    // Auto-Grow
                    if cursor + 4 + size >= file_len as usize {
                        mmap_opt.as_ref().unwrap().flush().unwrap();
                        mmap_opt = None;
                        let new_len = file_len + (1024 * 1024 * 1024);
                        file.set_len(new_len).unwrap();
                        file_len = new_len;
                        mmap_opt = Some(unsafe { MmapMut::map_mut(&file).unwrap() });
                    }

                    let mmap = mmap_opt.as_mut().unwrap();
                    mmap[cursor..cursor + 4].copy_from_slice(&(size as u32).to_le_bytes());
                    cursor += 4;
                    mmap[cursor..cursor + size].copy_from_slice(buf);
                    cursor += size;

                    unsynced_writes += 1;
                    if unsynced_writes >= SYNC_BATCH {
                        let _ = mmap.flush_async();
                        unsynced_writes = 0;
                    }
                }
                Err(_) => {
                    if let Some(m) = mmap_opt.as_ref() { let _ = m.flush(); }
                    break;
                }
            }
        }
    }

    pub fn append(&mut self, entry: LogEntry) {
        let _ = self.tx.send(entry);
        self.current_seq += 1;
    }
}

pub struct MatchingEngine {
    pub orders: FxHashMap<Ulid, Order>,
    pub match_sequence: u64,
    pub trade_history: Vec<Trade>,
    pub wal: Wal, // Assumes you kept the Wal struct from previous steps
    pub snapshot_dir: PathBuf,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        // [Startup logic same as before...]
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
        self.wal.append(LogEntry::PlaceOrder(order));
        self.orders.insert(order.id, order);
    }

    /// Simulate matching two orders and create a trade
    /// In a real matching engine, this would be called automatically when orders can be matched
    pub fn match_orders(&mut self, buy_order_id: Ulid, sell_order_id: Ulid, price: u64, quantity: u64) -> Option<Trade> {
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
        self.wal.append(LogEntry::Trade(trade));
        
        // Add to trade history
        self.trade_history.push(trade);

        println!("Trade Executed: match_id={}, buy={}, sell={}, price={}, qty={}", 
            trade.match_id, buy_order_id, sell_order_id, price, quantity);

        Some(trade)
    }

    pub fn cancel_order(&mut self, order_id: Ulid) -> bool {
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
        // unsafe: Forking is technically unsafe in multi-threaded apps,
        // but since our child only does file I/O and exits, it is generally safe here.
        match unsafe { fork() } {
            Ok(ForkResult::Parent { child: _ }) => {
                // --- PARENT PROCESS ---
                // Returns IMMEDIATELY (< 1ms).
                // The OS handles memory isolation lazily.
                // We do NOT wait for the child. We keep processing orders.

                // Optional: You might want to reap zombies periodically using waitpid(WNOHANG)
                // in a real loop, but for this demo, we ignore it.
            }
            Ok(ForkResult::Child) => {
                // --- CHILD PROCESS ---
                // We have an exact copy of `self.orders` at this instant.
                // The WAL background thread DOES NOT exist here (only calling thread survives fork).

                let start = Instant::now();
                let filename = format!("snapshot_{}.snap", current_seq);
                let path = snap_dir.join(filename);

                let snap = Snapshot {
                    last_seq: current_seq,
                    match_sequence: self.match_sequence,
                    orders: self.orders.clone(), // This is just a cheap struct copy in the child's isolated memory
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

                // CRITICAL: Child must exit immediately.
                // Do not let it return to the main loop!
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
        let order = Order {
            id: Ulid::new(),
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