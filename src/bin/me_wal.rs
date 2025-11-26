use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
// removed bail unused
use ulid::Ulid;

// ==========================================
// 1. DOMAIN & WAL TYPES (Unchanged)
// ==========================================

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum OrderSide { Buy, Sell }

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Order {
    pub id: Ulid,
    pub symbol: String,
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid },
}

#[derive(Serialize, Deserialize, Debug)]
struct WalFrame {
    pub seq: u64,
    pub entry: LogEntry,
}

#[derive(Serialize, Deserialize, Debug)]
struct Snapshot {
    pub last_seq: u64,
    pub orders: HashMap<Ulid, Order>,
}

// ==========================================
// 2. WAL IMPLEMENTATION (Unchanged)
// ==========================================

pub struct Wal {
    file: File,
    writer: BufWriter<File>,
    pub current_seq: u64,
}

impl Wal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true).read(true).append(true)
            .open(path).context("Failed to open WAL file")?;
        let writer_file = file.try_clone()?;
        Ok(Self { file, writer: BufWriter::new(writer_file), current_seq: 0 })
    }

    pub fn append(&mut self, entry: LogEntry) -> Result<()> {
        self.current_seq += 1;
        let frame = WalFrame { seq: self.current_seq, entry };
        let encoded = bincode::serialize(&frame)?;
        let len = encoded.len() as u64;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&encoded[..])?;
        self.writer.flush()?;
        // In real High Frequency Trading, you might batch sync_all
        self.file.sync_all()?;
        Ok(())
    }

    pub fn replay(&mut self) -> Result<Vec<(u64, LogEntry)>> {
        let mut file = &self.file;
        let mut entries = Vec::new();
        let mut reader = BufReader::new(file.try_clone()?);
        reader.seek(SeekFrom::Start(0))?;

        loop {
            let mut len_buf = [0u8; 8];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {
                    let len = u64::from_le_bytes(len_buf) as usize;
                    let mut payload = vec![0u8; len];
                    reader.read_exact(&mut payload)?;
                    let frame: WalFrame = bincode::deserialize(&payload[..])?;
                    entries.push((frame.seq, frame.entry));
                    if frame.seq > self.current_seq { self.current_seq = frame.seq; }
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(entries)
    }
}

// ==========================================
// 3. MATCHING ENGINE (UPDATED)
// ==========================================

pub struct MatchingEngine {
    pub orders: HashMap<Ulid, Order>,
    pub wal: Wal,
    pub snapshot_dir: PathBuf, // Changed from String path to PathBuf directory
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        // Ensure snapshot directory exists
        if !snapshot_dir.exists() {
            fs::create_dir_all(snapshot_dir)?;
        }

        let mut orders = HashMap::new();
        let mut recovered_seq = 0;

        // --- NEW RECOVERY LOGIC ---
        // 1. Scan directory for the highest sequence snapshot
        let latest_snap = Self::find_latest_snapshot(snapshot_dir)?;

        if let Some((seq, path)) = latest_snap {
            println!("   [System] Found latest snapshot: {:?} (Seq: {})", path, seq);
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            let snap: Snapshot = bincode::deserialize_from(reader)?;

            orders = snap.orders;
            recovered_seq = snap.last_seq;
            println!("   [System] State restored from Snapshot.");
        } else {
            println!("   [System] No snapshots found. Starting fresh.");
        }
        // --------------------------

        let mut wal = Wal::open(wal_path)?;

        // 2. Replay WAL (Filtering out old events)
        let history = wal.replay()?;
        let mut applied_count = 0;

        for (seq, entry) in history {
            if seq > recovered_seq {
                match entry {
                    LogEntry::PlaceOrder(o) => { orders.insert(o.id, o); }
                    LogEntry::CancelOrder { id } => { orders.remove(&id); }
                }
                applied_count += 1;
                if seq > recovered_seq { recovered_seq = seq; }
            }
        }

        wal.current_seq = recovered_seq;
        println!("   [System] WAL Replay Complete. Applied {} new events.", applied_count);

        Ok(Self {
            orders,
            wal,
            snapshot_dir: snapshot_dir.to_path_buf(),
        })
    }

    /// Helper to find the file with the highest sequence number
    fn find_latest_snapshot(dir: &Path) -> Result<Option<(u64, PathBuf)>> {
        let mut max_seq = 0;
        let mut found_path = None;

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Basic parsing: looks for "snapshot_{NUMBER}.snap"
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.starts_with("snapshot_") && path.extension().map_or(false, |e| e == "snap") {
                    let num_part = &stem["snapshot_".len()..];
                    if let Ok(seq) = num_part.parse::<u64>() {
                        if seq > max_seq {
                            max_seq = seq;
                            found_path = Some((seq, path));
                        }
                    }
                }
            }
        }
        Ok(found_path)
    }

    pub fn create_snapshot(&self) -> Result<()> {
        let current_seq = self.wal.current_seq;

        // --- NEW NAMING LOGIC ---
        let filename = format!("snapshot_{}.snap", current_seq);
        let file_path = self.snapshot_dir.join(filename);
        // ------------------------

        let snap = Snapshot {
            last_seq: current_seq,
            orders: self.orders.clone(),
        };

        let file = File::create(&file_path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, &snap)?;

        // We do NOT delete old snapshots here, as requested.

        Ok(())
    }

    pub fn place_order(&mut self, order: Order) -> Result<Ulid> {
        let id = order.id;
        self.wal.append(LogEntry::PlaceOrder(order.clone()))?;
        self.orders.insert(order.id, order);
        Ok(id)
    }
}

// ==========================================
// 4. MAIN (DEMO)
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("history_test.wal");
    let snap_dir = Path::new("snapshots_data"); // Directory, not file

    // cleanup
    if wal_path.exists() { fs::remove_file(wal_path)?; }
    if snap_dir.exists() { fs::remove_dir_all(snap_dir)?; }

    println!(">>> PHASE 1: Generating Orders & Multiple Snapshots");
    {
        let mut engine = MatchingEngine::new(wal_path, snap_dir)?;

        for i in 1..=3500 {
            engine.place_order(Order {
                id: Ulid::new(),
                symbol: "BTC".into(),
                side: OrderSide::Buy,
                price: 100,
                quantity: 1,
            })?;

            // Take snapshot every 1000 orders
            // Should create: snapshot_1000.snap, snapshot_2000.snap, snapshot_3000.snap
            if i % 1000 == 0 {
                engine.create_snapshot()?;
                println!("    -> Saved snapshot for Seq {}", i);
            }
        }
        println!("    Phase 1 Complete. Final Seq: {}", engine.wal.current_seq);
    }

    println!("\n>>> PHASE 2: Recovery");
    {
        // This should automatically find 'snapshot_3000.snap' and ignore 1000/2000
        let engine = MatchingEngine::new(wal_path, snap_dir)?;

        println!("    Current Seq in Engine: {}", engine.wal.current_seq);
        println!("    Total Orders Loaded:   {}", engine.orders.len());

        if engine.wal.current_seq == 3500 {
            println!("✅ SUCCESS: Recovered from snapshot_3000 + 500 WAL events.");
        } else {
            println!("❌ FAIL: Sequence mismatch.");
        }
    }

    Ok(())
}