// src/bin/me_wal.rs

// =================================================================
// 1. MODULE IMPORT (Select one method based on your setup)
// =================================================================

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
// FlatBuffers imports
use flatbuffers::FlatBufferBuilder;
use memmap2::{Mmap, MmapMut};
use rustc_hash::FxHashMap;
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

// METHOD A: Use this if build.rs is working (Recommended based on your logs)
#[allow(dead_code, unused_imports)]
mod wal_schema {
    include!(concat!(env!("OUT_DIR"), "/wal_generated.rs"));
}

// METHOD B: Use this if you manually pasted wal_generated.rs into src/bin/
// #[allow(dead_code, unused_imports)]
// #[path = "wal_generated.rs"]
// mod wal_schema;

// =================================================================
// 2. IMPORTS
// =================================================================

// ==========================================
// 3. DOMAIN TYPES (In-Memory)
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq)]
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

// Helper: Rust Ulid -> FlatBuffer UlidStruct
fn to_fbs_ulid(u: Ulid) -> UlidStruct {
    let n = u.0;
    let hi = (n >> 64) as u64;
    let lo = n as u64;
    UlidStruct::new(hi, lo)
}

// Helper: FlatBuffer UlidStruct -> Rust Ulid
fn from_fbs_ulid(f: &UlidStruct) -> Ulid {
    let hi = f.hi() as u128;
    let lo = f.lo() as u128;
    Ulid((hi << 64) | lo)
}

// ==========================================
// 4. WAL IMPLEMENTATION
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

        // 1GB Allocation
        let file_len = 1024 * 1024 * 1024;
        if file.metadata().unwrap().len() < file_len { file.set_len(file_len).unwrap(); }

        let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
        let mut builder = FlatBufferBuilder::new();

        // Scan for end of file
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

                    // --- ASSIGNMENT FIX HERE ---
                    // We capture the result of the match into variables
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
                            let cancel = Cancel::create(&mut builder, &CancelArgs {
                                id: Some(&f_ulid),
                            });
                            (EntryType::Cancel, cancel.as_union_value())
                        }
                    };

                    // Create the Frame using the variables we just assigned
                    let frame = WalFrame::create(&mut builder, &WalFrameArgs {
                        seq: current_seq,
                        entry_type,
                        entry: Some(entry_offset),
                    });

                    builder.finish(frame, None);
                    let buf = builder.finished_data();
                    let size = buf.len();

                    // Check Bounds
                    if cursor + 4 + size >= file_len as usize { panic!("WAL Full"); }

                    // Write Length (u32)
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
                    let _ = mmap.flush();
                    break;
                }
            }
        }
    }

    pub fn replay(path: &Path) -> Result<(Vec<LogEntry>, u64)> {
        if !path.exists() { return Ok((Vec::new(), 0)); }
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
            let frame = wal_schema::wal_schema::root_as_wal_frame(data)?;

            if frame.seq() > max_seq { max_seq = frame.seq(); }

            if let Some(order) = frame.entry_as_order() {
                let f_id = order.id().unwrap();
                let f_sym = order.symbol().unwrap();
                let f_side = order.side();

                let mut sym_arr = [0u8; 8];
                let bytes = f_sym.as_bytes();
                let copy_len = bytes.len().min(8);
                sym_arr[..copy_len].copy_from_slice(&bytes[..copy_len]);

                entries.push(LogEntry::PlaceOrder(Order {
                    id: from_fbs_ulid(f_id),
                    symbol: sym_arr,
                    side: if f_side == FbsSide::Buy { OrderSide::Buy } else { OrderSide::Sell },
                    price: order.price(),
                    quantity: order.quantity(),
                }));
            }
            // Add Cancel replay logic here if needed

            cursor += len;
        }

        Ok((entries, max_seq))
    }

    pub fn append(&mut self, entry: LogEntry) {
        let _ = self.tx.send(entry);
        self.current_seq += 1;
    }
}

// ==========================================
// 5. ENGINE & MAIN
// ==========================================

pub struct MatchingEngine {
    pub orders: FxHashMap<Ulid, Order>,
    pub wal: Wal,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path) -> Result<Self> {
        let start = Instant::now();
        let (history, seq) = Wal::replay(wal_path)?;
        println!("Replayed {} events in {:.2?}", history.len(), start.elapsed());

        let mut orders = FxHashMap::default();
        for entry in history {
            match entry {
                LogEntry::PlaceOrder(o) => { orders.insert(o.id, o); }
                _ => {}
            }
        }

        Ok(Self {
            orders,
            wal: Wal::open(wal_path, seq)?,
        })
    }

    pub fn place_order(&mut self, order: Order) {
        self.wal.append(LogEntry::PlaceOrder(order));
        self.orders.insert(order.id, order);
    }
}

fn main() -> Result<()> {
    let path = Path::new("flatbuffer.wal");
    // Clean up previous run
    if path.exists() { fs::remove_file(path)?; }

    let mut engine = MatchingEngine::new(path)?;

    let total = 1_000_000;
    println!("Generating {} orders...", total);
    let start = Instant::now();

    for _ in 0..total {
        engine.place_order(Order {
            id: Ulid::new(),
            symbol: *b"ETH_USDT",
            side: OrderSide::Buy,
            price: 2000,
            quantity: 1,
        });
    }

    let duration = start.elapsed();
    println!("Written in {:.2?}", duration);
    println!("TPS: {:.0}", total as f64 / duration.as_secs_f64());

    // Give IO thread time to flush before exiting
    thread::sleep(Duration::from_secs(1));
    Ok(())
}