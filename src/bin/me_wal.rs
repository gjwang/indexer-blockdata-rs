use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use memmap2::MmapMut;
use serde::{Deserialize, Serialize};
// Allocator is critical for HashMap performance
#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;
use ulid::Ulid;
use fetcher::fast_ulid::FastUlidGenerator;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;



// ==========================================
// 1. ZERO-COST HASHER (The Speed Trick)
// ==========================================
// ULIDs are random. We don't need to mix bits.
// We just use the first 64 bits of the ULID as the hash.

#[derive(Default)]
struct IdentityHasher(u64);

impl Hasher for IdentityHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("Should only hash u128/u64 directly")
    }

    // We assume the key is u128 (ULID)
    fn write_u128(&mut self, i: u128) {
        // Just take the lower 64 bits. Zero CPU cost.
        self.0 = i as u64;
    }

    // Fallback if we use u64 keys
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }

    fn finish(&self) -> u64 { self.0 }
}

// BuildHasher implementation
#[derive(Default, Clone, Copy)]
struct IdentityBuildHasher;
impl BuildHasher for IdentityBuildHasher {
    type Hasher = IdentityHasher;
    fn build_hasher(&self) -> Self::Hasher { IdentityHasher(0) }
}

// ==========================================
// 2. RAW DOMAIN TYPES
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum OrderSide { Buy = 0, Sell = 1 }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[repr(C)]
pub struct Order {
    // We use u128 to represent ULID in raw memory for speed/simplicity
    pub id: u128,
    pub symbol: [u8; 8],
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
    pub _pad: [u8; 7],
}

const BATCH_SIZE: usize = 1024;
struct WalBatch {
    count: usize,
    entries: Box<[Order; BATCH_SIZE]>,
}

// ==========================================
// 3. WAL (Same Raw Mmap Writer)
// ==========================================
// Simple background thread that copies bytes to disk.

struct Wal {
    tx: Sender<WalBatch>,
    local_batch: Box<[Order; BATCH_SIZE]>,
    local_count: usize,
}

impl Wal {
    fn open(path: &Path) -> Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = bounded(2000);

        thread::spawn(move || Self::background_writer(path, rx));

        Ok(Self {
            tx,
            local_batch: Box::new([unsafe { mem::zeroed() }; BATCH_SIZE]),
            local_count: 0,
        })
    }

    fn background_writer(path: PathBuf, rx: Receiver<WalBatch>) {
        let file = OpenOptions::new().read(true).write(true).create(true).open(&path).unwrap();
        let mut len = 1024 * 1024 * 1024; // 1GB
        if file.metadata().unwrap().len() < len { file.set_len(len).unwrap(); }

        let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
        let mut cursor = 0;

        // Skip existing
        while cursor + mem::size_of::<Order>() < len as usize && mmap[cursor] != 0 {
            cursor += mem::size_of::<Order>();
        }

        while let Ok(batch) = rx.recv() {
            let size = batch.count * mem::size_of::<Order>();

            // Auto-Grow
            if cursor + size >= len as usize {
                mmap.flush().unwrap();
                let new_len = len * 2;
                file.set_len(new_len as u64).unwrap();
                len = new_len;
                mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            }

            // Raw Memcpy
            unsafe {
                let src = batch.entries.as_ptr() as *const u8;
                let dst = mmap.as_mut_ptr().add(cursor);
                std::ptr::copy_nonoverlapping(src, dst, size);
            }
            cursor += size;
        }
    }

    #[inline(always)]
    fn append(&mut self, order: Order) {
        unsafe {
            *self.local_batch.get_unchecked_mut(self.local_count) = order;
        }
        self.local_count += 1;

        if self.local_count == BATCH_SIZE {
            let _ = self.tx.send(WalBatch {
                count: BATCH_SIZE,
                entries: self.local_batch.clone(),
            });
            self.local_count = 0;
        }
    }

    fn flush(&mut self) {
        if self.local_count > 0 {
            let _ = self.tx.send(WalBatch {
                count: self.local_count,
                entries: self.local_batch.clone(),
            });
            self.local_count = 0;
        }
    }
}

// ==========================================
// 4. ENGINE (Single Threaded Optimized)
// ==========================================

pub struct MatchingEngine {
    // IdentityHasher makes this map much faster for ULIDs
    pub orders: HashMap<u128, Order, IdentityBuildHasher>,
    pub wal: Wal,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, capacity: usize) -> Result<Self> {
        if wal_path.exists() { fs::remove_file(wal_path)?; }

        // Pre-allocate to prevent resizing lag
        let orders = HashMap::with_capacity_and_hasher(capacity, IdentityBuildHasher);

        Ok(Self {
            orders,
            wal: Wal::open(wal_path)?,
        })
    }

    #[inline(always)]
    pub fn place_order(&mut self, order: Order) {
        self.wal.append(order);
        self.orders.insert(order.id, order);
    }
}

// ==========================================
// 5. MAIN
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("ulid_single.wal");
    let total = 100_000;

    println!(">>> STARTING SIMPLE ULID ENGINE (No Sharding)");
    println!("    Orders: {}", total);

    let mut engine = MatchingEngine::new(wal_path, total)?;
    let symbol = *b"BTC_USDT";

    // Pre-generate IDs (Simulating Gateway)
    // We don't want to measure Ulid generation time, only Engine Insert time.
    println!("    Generating IDs...");
    let t_id = Instant::now();
    let mut ids = Vec::with_capacity(total);

    // Initialize the Monotonic Generator ONCE per thread
    let mut ulid_gen = FastUlidGenerator::new();
    for _ in 0..total { ids.push(ulid_gen.generate().0); }
    // for _ in 0..total { ids.push(Ulid::new().0); }

    println!("    Generating IDs: {:?}", t_id.elapsed());


    println!("    Processing...");
    let start = Instant::now();

    for id in ids {
        engine.place_order(Order {
            id,
            symbol,
            side: OrderSide::Buy,
            price: 100,
            quantity: 1,
            _pad: [0; 7],
        });
    }

    engine.wal.flush();

    let dur = start.elapsed();
    let seconds = dur.as_secs_f64();

    println!("\n>>> RESULTS:");
    println!("    Time:       {:.4} s", seconds);
    println!("    Throughput: {:.0} orders/sec", total as f64 / seconds);

    thread::sleep(Duration::from_secs(1));
    Ok(())
}