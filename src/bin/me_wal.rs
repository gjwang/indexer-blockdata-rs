// =================================================================
// 1. MODULE IMPORT
// =================================================================

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
// FlatBuffers imports
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use memmap2::{Mmap, MmapMut};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use wal_schema::wal_schema::{
    Cancel,
    CancelArgs,
    EntryType,
    Order as FbsOrder,
    OrderArgs,
    OrderSide as FbsSide,
    UlidStruct,
    WalFrame,
    WalFrameArgs,
};

// Adjust this depending on where your generated code lives.
// If you manually created src/bin/wal_generated.rs, keep the #[path] line.
#[allow(dead_code, unused_imports)]
#[path = "wal_generated.rs"]
mod wal_schema;

// =================================================================
// 2. IMPORTS
// =================================================================

// For Snapshot only

// ==========================================
// 3. DOMAIN TYPES (In-Memory)
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: Ulid,
    pub symbol: [u8; 8], // Stack allocated string
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid },
}

// SNAPSHOT STRUCT (Uses Bincode for simple full-state dump)
#[derive(Serialize, Deserialize)]
struct Snapshot {
    pub last_seq: u64,
    pub orders: FxHashMap<Ulid, Order>,
}

// Helpers
fn to_fbs_ulid(u: Ulid) -> UlidStruct {
    let n = u.0;
    UlidStruct::new((n >> 64) as u64, n as u64)
}

fn from_fbs_ulid(f: &UlidStruct) -> Ulid {
    Ulid(((f.hi() as u128) << 64) | (f.lo() as u128))
}

// ==========================================
// 4. WAL IMPLEMENTATION (FlatBuffers + Mmap)
// ==========================================

pub struct Wal {
    tx: Sender<LogEntry>,
    pub current_seq: u64,
}

impl Wal {
    pub fn open(path: &Path, start_seq: u64) -> Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = bounded(1_000_000);

        thread::spawn(move || {
            Self::background_writer(path, rx, start_seq);
        });

        Ok(Self { tx, current_seq: start_seq })
    }

    fn background_writer(path: PathBuf, rx: Receiver<LogEntry>, mut current_seq: u64) {
        let file = OpenOptions::new().read(true).write(true).create(true).open(&path).unwrap();

        // Pre-allocate 1GB
        let file_len = 1024 * 1024 * 1024;
        if file.metadata().unwrap().len() < file_len { file.set_len(file_len).unwrap(); }

        let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
        let mut builder = FlatBufferBuilder::new();

        // Scan for end
        let mut cursor = 0;
        while cursor + 4 < file_len as usize {
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let len = u32::from_le_bytes(len_bytes) as usize;
            if len == 0 { break; }
            cursor += 4 + len;
        }

        const SYNC_BATCH: usize = 2000;
        let mut unsynced_writes = 0;

        loop {
            match rx.recv() {
                Ok(entry) => {
                    current_seq += 1;
                    builder.reset();

                    // --- 1. Construct FlatBuffer ---
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
                            });
                            (EntryType::Order, order.as_union_value())
                        }
                        LogEntry::CancelOrder { id } => {
                            let f_ulid = to_fbs_ulid(id);
                            let cancel = Cancel::create(&mut builder, &CancelArgs { id: Some(&f_ulid) });
                            (EntryType::Cancel, cancel.as_union_value())
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

                    if cursor + 4 + size >= file_len as usize { panic!("WAL Full"); }

                    // --- 2. Write to Mmap ---
                    mmap[cursor..cursor + 4].copy_from_slice(&(size as u32).to_le_bytes());
                    cursor += 4;
                    mmap[cursor..cursor + size].copy_from_slice(buf);
                    cursor += size;

                    // --- 3. Async Sync ---
                    unsynced_writes += 1;
                    if unsynced_writes >= SYNC_BATCH {
                        let _ = mmap.flush_async();
                        unsynced_writes = 0;
                    }
                }
                Err(_) => {
                    let _ = mmap.flush();
                    break;
                }
            }
        }
    }

    // Returns (Entries, MaxSeq, Duration)
    pub fn replay(path: &Path, min_seq: u64) -> Result<(Vec<LogEntry>, u64, Duration)> {
        let start = Instant::now();
        if !path.exists() { return Ok((Vec::new(), 0, Duration::ZERO)); }

        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        let mut entries = Vec::new();
        let mut max_seq = 0;
        let mut cursor = 0;

        while cursor + 4 < mmap.len() {
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let len = u32::from_le_bytes(len_bytes) as usize;
            if len == 0 { break; }
            cursor += 4;

            let data = &mmap[cursor..cursor + len];
            // Zero-Copy Verify
            let frame = wal_schema::wal_schema::root_as_wal_frame(data)?;
            let frame_seq = frame.seq();

            if frame_seq > max_seq { max_seq = frame_seq; }

            // Only deserialize if we need it (newer than snapshot)
            if frame_seq > min_seq {
                if let Some(order) = frame.entry_as_order() {
                    let mut sym_arr = [0u8; 8];
                    let bytes = order.symbol().unwrap().as_bytes();
                    let copy_len = bytes.len().min(8);
                    sym_arr[..copy_len].copy_from_slice(&bytes[..copy_len]);

                    entries.push(LogEntry::PlaceOrder(Order {
                        id: from_fbs_ulid(order.id().unwrap()),
                        symbol: sym_arr,
                        side: if order.side() == FbsSide::Buy { OrderSide::Buy } else { OrderSide::Sell },
                        price: order.price(),
                        quantity: order.quantity(),
                    }));
                }
            }

            cursor += len;
        }

        Ok((entries, max_seq, start.elapsed()))
    }

    pub fn append(&mut self, entry: LogEntry) {
        let _ = self.tx.send(entry);
        self.current_seq += 1;
    }
}

// ==========================================
// 5. MATCHING ENGINE
// ==========================================

pub struct MatchingEngine {
    pub orders: FxHashMap<Ulid, Order>,
    pub wal: Wal,
    pub snapshot_dir: PathBuf,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        let boot_start = Instant::now();
        fs::create_dir_all(snapshot_dir)?;

        let mut orders = FxHashMap::default();
        let mut recovered_seq = 0;

        // --- 1. LOAD SNAPSHOT ---
        let snap_start = Instant::now();
        if let Some((seq, path)) = Self::find_latest_snapshot(snapshot_dir)? {
            println!("   [Recover] Found Snapshot: {:?} (Seq {})", path, seq);
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            let snap: Snapshot = bincode::deserialize_from(reader)?;
            orders = snap.orders;
            recovered_seq = snap.last_seq;
            println!("   [Metrics] Snapshot Load:      {:.2?}", snap_start.elapsed());
        }

        // --- 2. REPLAY WAL ---
        println!("   [Recover] Scanning FlatBuffers WAL > Seq {}...", recovered_seq);
        let (entries, file_max, wal_dur) = Wal::replay(wal_path, recovered_seq)?;

        let apply_start = Instant::now();
        for entry in entries {
            match entry {
                LogEntry::PlaceOrder(o) => { orders.insert(o.id, o); }
                _ => {}
            }
        }

        println!("   [Metrics] WAL Scan:           {:.2?}", wal_dur);
        println!("   [Metrics] Memory Apply:       {:.2?}", apply_start.elapsed());
        println!("   [Metrics] TOTAL RECOVERY:     {:.2?}", boot_start.elapsed());

        Ok(Self {
            orders,
            wal: Wal::open(wal_path, std::cmp::max(recovered_seq, file_max))?,
            snapshot_dir: snapshot_dir.to_path_buf(),
        })
    }

    fn find_latest_snapshot(dir: &Path) -> Result<Option<(u64, PathBuf)>> {
        let mut max_seq = 0;
        let mut found = None;
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.starts_with("snapshot_") && path.extension().map_or(false, |e| e == "snap") {
                    if let Ok(seq) = stem["snapshot_".len()..].parse::<u64>() {
                        if seq > max_seq {
                            max_seq = seq;
                            found = Some((seq, path));
                        }
                    }
                }
            }
        }
        Ok(found)
    }

    pub fn create_snapshot(&self) -> Result<()> {
        let start = Instant::now();
        let seq = self.wal.current_seq;
        let filename = format!("snapshot_{}.snap", seq);
        let path = self.snapshot_dir.join(filename);

        let snap = Snapshot {
            last_seq: seq,
            orders: self.orders.clone(),
        };

        let file = File::create(&path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, &snap)?;

        println!("   [Metrics] Snapshot Saved:     {:.2?} (Seq: {})", start.elapsed(), seq);
        Ok(())
    }

    #[inline(always)]
    pub fn place_order(&mut self, order: Order) {
        self.wal.append(LogEntry::PlaceOrder(order));
        self.orders.insert(order.id, order);
    }
}

// ==========================================
// 6. MAIN BENCHMARK
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("nuclear.wal");
    let snap_dir = Path::new("nuclear_snaps");

    // Cleanup for clean test
    if wal_path.exists() { fs::remove_file(wal_path)?; }
    if snap_dir.exists() { fs::remove_dir_all(snap_dir)?; }

    let total = 1_000_000;

    println!(">>> PHASE 1: GENERATION ({} Orders)", total);
    {
        let mut engine = MatchingEngine::new(wal_path, snap_dir)?;
        let start = Instant::now();

        for i in 1..=total {
            engine.place_order(Order {
                id: Ulid::new(),
                symbol: *b"ETH_USDT",
                side: OrderSide::Buy,
                price: 2000,
                quantity: 1,
            });

            // Trigger snapshot at 50%
            if i == total / 2 {
                engine.create_snapshot()?;
            }
        }

        let dur = start.elapsed();
        println!("\n>>> WRITE METRICS:");
        println!("    Total Time:   {:.2?}", dur);
        println!("    Throughput:   {:.0} orders/sec", total as f64 / dur.as_secs_f64());

        thread::sleep(Duration::from_millis(500)); // Let IO flush
    }

    println!("\n>>> PHASE 2: RECOVERY");
    {
        // This will print the [Metrics] lines defined in Engine::new
        let engine = MatchingEngine::new(wal_path, snap_dir)?;

        println!("\n>>> VALIDATION:");
        println!("    Memory Orders: {}", engine.orders.len());
        println!("    Expected:      {}", total);

        if engine.orders.len() == total {
            println!("✅ SUCCESS");
        } else {
            println!("❌ FAILURE");
        }
    }

    Ok(())
}