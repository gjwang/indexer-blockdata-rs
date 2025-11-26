use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use memmap2::MmapMut;

// ==========================================
// 1. DOMAIN TYPES (Optimized)
// ==========================================

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum OrderSide { Buy, Sell }

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct Order {
    pub id: Ulid,
    pub symbol: [u8; 8], // Stack allocated string
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid },
}

#[derive(Serialize, Deserialize, Debug)]
struct WalFrame {
    pub seq: u64,
    pub entry: LogEntry,
}

// ==========================================
// 2. MMAP WAL IMPLEMENTATION
// ==========================================

pub struct Wal {
    tx: Sender<LogEntry>,
    pub current_seq: u64,
}

impl Wal {
    pub fn open(path: &Path, start_seq: u64) -> Result<Self> {
        let path = path.to_path_buf();
        // Deep buffer to absorb spikes
        let (tx, rx) = bounded(1_000_000);

        thread::spawn(move || {
            Self::background_mmap_writer(path, rx, start_seq);
        });

        Ok(Self {
            tx,
            current_seq: start_seq,
        })
    }

    fn background_mmap_writer(path: PathBuf, rx: Receiver<LogEntry>, mut current_seq: u64) {
        // 1. OPEN & PRE-ALLOCATE FILE
        let file = OpenOptions::new()
            .read(true).write(true).create(true)
            .open(&path).expect("Failed to open WAL");

        // CRITICAL: We MUST pre-allocate the file size.
        // allocating 1GB (1024*1024*1024).
        // In prod, you check file len and extend if needed.
        let file_len = 1024 * 1024 * 1024;
        file.set_len(file_len).expect("Failed to allocate WAL file");

        // 2. CREATE MEMORY MAP
        // unsafe: We promise not to access this memory incorrectly.
        let mut mmap = unsafe { MmapMut::map_mut(&file).expect("Failed to mmap") };

        // 3. LOCATE STARTING OFFSET
        // If restarting, we need to scan (skip for this speed demo, assume 0 or passed offset)
        // For this demo, we assume we start writing at offset 0 (or you'd save offset in a header).
        let mut cursor = 0;

        // Skip existing data if we are appending (simple naive scan for non-zero bytes)
        // In prod, you save the 'cursor' position in a separate metadata file.
        while cursor < file_len as usize && mmap[cursor] != 0 {
            // This is a naive skip. Real implementations track offset separately.
            cursor += 1;
        }

        const SYNC_BATCH: usize = 2000;
        const SYNC_TIME: Duration = Duration::from_millis(20);

        let mut unsynced_writes = 0;
        let mut last_sync = Instant::now();

        // Reusable serialization buffer to avoid stack churn
        // Max frame size estimate: 8 bytes (seq) + 1 byte (variant) + 50 bytes (Order) ~ 64 bytes
        let mut scratch_buf = [0u8; 128];

        loop {
            let timeout = SYNC_TIME.checked_sub(last_sync.elapsed()).unwrap_or(Duration::ZERO);

            match rx.recv_timeout(timeout) {
                Ok(entry) => {
                    current_seq += 1;
                    let frame = WalFrame { seq: current_seq, entry };

                    // A. SERIALIZE TO STACK (Extremely fast)
                    // We serialize to a scratch buffer first to know the length
                    let size = bincode::serialized_size(&frame).unwrap() as usize;
                    bincode::serialize_into(&mut scratch_buf[..], &frame).unwrap();

                    // B. CHECK BOUNDS
                    if cursor + 8 + size >= file_len as usize {
                        panic!("WAL FULL! Implement rotation or extension.");
                    }

                    // C. WRITE LEN + DATA DIRECTLY TO RAM (memcpy)
                    // Write 8 bytes length
                    let len_bytes = (size as u64).to_le_bytes();

                    // unsafe copy is slightly faster, but slice copy is safe and usually optimized to memcpy
                    mmap[cursor..cursor + 8].copy_from_slice(&len_bytes);
                    cursor += 8;

                    // Write payload
                    mmap[cursor..cursor + size].copy_from_slice(&scratch_buf[..size]);
                    cursor += size;

                    unsynced_writes += 1;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if unsynced_writes > 0 {
                        // Async flush: tells OS "start writing dirty pages to disk"
                        // It doesn't block heavily.
                        let _ = mmap.flush_async();
                        unsynced_writes = 0;
                        last_sync = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = mmap.flush(); // Blocking sync on exit
                    break;
                }
            }

            // Periodic Sync
            if unsynced_writes >= SYNC_BATCH {
                // flush_async() is the magic. It initiates IO but returns quickly.
                // It ensures durability without stalling the thread like fsync().
                let _ = mmap.flush_async();
                unsynced_writes = 0;
                last_sync = Instant::now();
            }
        }
    }

    #[inline(always)]
    pub fn append(&mut self, entry: LogEntry) {
        let _ = self.tx.send(entry);
        self.current_seq += 1;
    }
}

// ==========================================
// 3. MATCHING ENGINE
// ==========================================

pub struct MatchingEngine {
    pub orders: FxHashMap<Ulid, Order>,
    pub wal: Wal,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path) -> Result<Self> {
        let wal = Wal::open(wal_path, 0)?;
        Ok(Self {
            orders: FxHashMap::default(),
            wal,
        })
    }

    #[inline(always)]
    pub fn place_order(&mut self, order: Order) {
        self.wal.append(LogEntry::PlaceOrder(order));
        self.orders.insert(order.id, order);
    }
}

// ==========================================
// 4. MAIN BENCHMARK
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("mmap.wal");
    if wal_path.exists() { fs::remove_file(wal_path)?; }

    let total_orders = 5_000_000; // 5 MILLION ORDERS

    println!(">>> STARTING MMAP NUCLEAR TEST ({} Orders)", total_orders);

    let mut engine = MatchingEngine::new(wal_path)?;
    let symbol = *b"BTCUSDT\0";

    // Warmup
    thread::sleep(Duration::from_millis(100));

    let start = Instant::now();

    for _ in 0..total_orders {
        engine.place_order(Order {
            id: Ulid::new(),
            symbol,
            side: OrderSide::Buy,
            price: 100,
            quantity: 1,
        });
    }

    let duration = start.elapsed();
    let ops = total_orders as f64 / duration.as_secs_f64();

    println!("\n>>> RESULTS (MMAP):");
    println!("    Total Time:    {:.2?}", duration);
    println!("    Orders/Sec:    {:.0} orders/sec", ops);

    // Give time for async flush to finish
    thread::sleep(Duration::from_secs(1));

    Ok(())
}