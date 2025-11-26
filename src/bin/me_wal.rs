use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
// FlatBuffers
use flatbuffers::FlatBufferBuilder;
use im::HashMap as ImHashMap;
use memmap2::MmapMut;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use wal_schema::wal_schema::{
    Cancel, CancelArgs, EntryType, Order as FbsOrder, OrderArgs,
    OrderSide as FbsSide, UlidStruct, WalFrame, WalFrameArgs,
};

#[allow(dead_code, unused_imports)]
mod wal_schema {
    include!(concat!(env!("OUT_DIR"), "/wal_generated.rs"));
}

// ==========================================
// 1. DOMAIN TYPES
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
}

#[derive(Debug, Clone, Copy)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid },
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    pub last_seq: u64,
    pub orders: ImHashMap<Ulid, Order>,
}

fn to_fbs_ulid(u: Ulid) -> UlidStruct {
    let n = u.0;
    UlidStruct::new((n >> 64) as u64, n as u64)
}

fn from_fbs_ulid(f: &UlidStruct) -> Ulid {
    Ulid(((f.hi() as u128) << 64) | (f.lo() as u128))
}

// ==========================================
// 2. WAL (Simple Mmap Writer)
// ==========================================
// This is much simpler now. It ONLY writes logs. It knows nothing about snapshots.

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

        // Start with 1GB
        let mut file_len = 1024 * 1024 * 1024;
        if file.metadata().unwrap().len() < file_len { file.set_len(file_len).unwrap(); }

        // We put mmap in an Option so we can drop it easily during resize
        let mut mmap_opt = Some(unsafe { MmapMut::map_mut(&file).unwrap() });
        let mut builder = FlatBufferBuilder::new();

        // Scan for end of data to find cursor position
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

        const SYNC_BATCH: usize = 2000;
        let mut unsynced_writes = 0;

        loop {
            match rx.recv() {
                Ok(entry) => {
                    current_seq += 1;
                    builder.reset();

                    // [Serialization Logic - Same as before]
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

                    // --- [FIX] DYNAMIC GROWTH LOGIC ---
                    if cursor + 4 + size >= file_len as usize {
                        // 1. Sync current data
                        mmap_opt.as_ref().unwrap().flush().unwrap();

                        // 2. DROP the current mmap (Unmap)
                        mmap_opt = None;

                        // 3. Extend File (Double the size)
                        let new_len = file_len * 2;
                        println!("   [WAL] Extending file from {} GB to {} GB...",
                                 file_len / 1024 / 1024 / 1024,
                                 new_len / 1024 / 1024 / 1024);

                        if let Err(e) = file.set_len(new_len) {
                            panic!("Failed to extend WAL file: {}", e);
                        }
                        file_len = new_len;

                        // 4. Re-Map
                        mmap_opt = Some(unsafe { MmapMut::map_mut(&file).unwrap() });
                    }
                    // ----------------------------------

                    let mmap = mmap_opt.as_mut().unwrap();

                    // Write Length
                    mmap[cursor..cursor + 4].copy_from_slice(&(size as u32).to_le_bytes());
                    cursor += 4;
                    // Write Data
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

// ==========================================
// 3. MATCHING ENGINE
// ==========================================

pub struct MatchingEngine {
    pub orders: ImHashMap<Ulid, Order>,
    pub wal: Wal,
    pub snapshot_dir: PathBuf,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        fs::create_dir_all(snapshot_dir)?;

        let mut orders = ImHashMap::default();
        let mut recovered_seq = 0;

        // 1. Load Snapshot (Simpler: just read the file)
        if let Some((seq, path)) = Self::find_latest_snapshot(snapshot_dir)? {
            println!("   [Recover] Loading Snapshot: {:?}", path);
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            let snap: Snapshot = bincode::deserialize_from(reader)?;
            orders = snap.orders;
            recovered_seq = snap.last_seq;
        }

        // 2. Replay WAL (We assume the standard replay logic exists or we skip implementing it here to save space)
        // ... (Insert Replay Logic Here if needed, same as previous versions) ...

        let wal = Wal::open(wal_path, recovered_seq)?;

        Ok(Self { orders, wal, snapshot_dir: snapshot_dir.to_path_buf() })
    }

    // Helper to find snap
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

    #[inline(always)]
    pub fn place_order(&mut self, order: Order) {
        self.wal.append(LogEntry::PlaceOrder(order));
        self.orders.insert(order.id, order);
    }

    // =========================================================
    // THE SIMPLIFIED "OFFLOAD" SNAPSHOT LOGIC
    // =========================================================
    pub fn trigger_snapshot_offload(&self) {
        // 1. CAPTURE STATE
        let start_clone = Instant::now();
        let current_seq = self.wal.current_seq;

        // This is the ONLY blocking part.
        // Cloning 1M items takes ~20ms-50ms (depending on RAM speed).
        // This is acceptable for 99% of engines that aren't High-Frequency Trading.
        let state_copy = self.orders.clone();
        let clone_dur = start_clone.elapsed();

        let dir = self.snapshot_dir.clone();

        // 2. SPAWN THREAD (Fire and Forget)
        thread::spawn(move || {
            let start_write = Instant::now();
            let filename = format!("snapshot_{}.snap", current_seq);
            let path = dir.join(filename);

            let snap = Snapshot {
                last_seq: current_seq,
                orders: state_copy, // We own this copy now
            };

            // Write to disk (Slow part happens here, NOT blocking main thread)
            if let Ok(file) = File::create(&path) {
                let writer = BufWriter::new(file);
                if let Err(e) = bincode::serialize_into(writer, &snap) {
                    println!("Error writing snapshot: {:?}", e);
                }
            }

            // Optional: Print metrics from background thread
            println!(
                "   [Background] Snapshot Saved (Seq: {}). Clone: {:.2?}, Write: {:.2?}",
                current_seq, clone_dur, start_write.elapsed()
            );
        });
    }
}

// ==========================================
// MAIN
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("simple.wal");
    let snap_dir = Path::new("simple_snaps");

    if wal_path.exists() { fs::remove_file(wal_path)?; }
    if snap_dir.exists() { fs::remove_dir_all(snap_dir)?; }

    let total = 10_000_000;
    println!(">>> STARTING SIMPLE CLONE-SNAPSHOT TEST ({} Orders)", total);

    let mut engine = MatchingEngine::new(wal_path, snap_dir)?;
    let start = Instant::now();
    let symbol = *b"ETH_USDT";

    for i in 1..=total {
        engine.place_order(Order {
            id: Ulid::new(),
            symbol,
            side: OrderSide::Buy,
            price: 100,
            quantity: 1,
        });

        if i % 100_000 == 0 {
            // This will block for ~30ms (Clone) then return immediately
            engine.trigger_snapshot_offload();
        }
    }

    let dur = start.elapsed();
    println!(">>> DONE");
    println!("    Total Time: {:.2?}", dur);
    println!("    Throughput: {:.0} orders/sec", total as f64 / dur.as_secs_f64());

    // Wait for background threads to finish writing
    thread::sleep(Duration::from_secs(2));

    Ok(())
}