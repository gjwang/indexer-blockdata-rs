use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

// ==========================================
// 1. DOMAIN TYPES
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

#[derive(Serialize, Deserialize, Debug, Clone)] // Added Clone for buffer
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
// 2. ASYNC WAL IMPLEMENTATION
// ==========================================

pub struct Wal {
    // We only hold the channel to send data to the background thread
    tx: Sender<LogEntry>,
    // We keep this just to read current_seq for snapshots/metrics (shared visually)
    // In a strict Rust setup, we'd use an Arc<AtomicU64>, but for this demo
    // we will just track seq in memory on the main thread too.
    pub current_seq: u64,
    // Join handle to wait for thread on shutdown
    worker_handle: Option<thread::JoinHandle<()>>,
}

impl Wal {
    pub fn open(path: &Path, start_seq: u64) -> Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = mpsc::channel();

        // Spawn the Background Writer Thread
        let handle = thread::spawn(move || {
            Self::background_writer(path, rx, start_seq);
        });

        Ok(Self {
            tx,
            current_seq: start_seq,
            worker_handle: Some(handle),
        })
    }

    // This loop runs on the separate thread
    // Inside impl Wal ...

    fn background_writer(path: PathBuf, rx: Receiver<LogEntry>, mut current_seq: u64) {
        let file = OpenOptions::new()
            .create(true).read(true).append(true)
            .open(&path).expect("Failed to open WAL");

        let mut writer = BufWriter::new(file);

        // SYNC CONFIGURATION
        const SYNC_BATCH_SIZE: u64 = 1000;       // Sync every 100 events
        const SYNC_TIMEOUT: Duration = Duration::from_millis(10); // Or every 10ms

        let mut unsynced_count = 0;
        let mut last_sync = Instant::now();

        loop {
            // 1. Wait for data (Blocking, but we wake up periodically to check sync time)
            // We set the timeout to the remaining time until the next forced sync
            let now = Instant::now();
            let time_since_sync = now.duration_since(last_sync);
            let time_remaining = SYNC_TIMEOUT.checked_sub(time_since_sync).unwrap_or(Duration::ZERO);

            match rx.recv_timeout(time_remaining) {
                Ok(entry) => {
                    // --- STEP A: WRITE TO OS KERNEL (IMMEDIATELY) ---
                    current_seq += 1;
                    let frame = WalFrame { seq: current_seq, entry };

                    if let Ok(encoded) = bincode::serialize(&frame) {
                        let len = encoded.len() as u64;
                        let _ = writer.write_all(&len.to_le_bytes());
                        let _ = writer.write_all(&encoded);

                        // CRITICAL: We flush to the OS immediately!
                        // This protects against App Crashes (Panic/Segfault).
                        let _ = writer.flush();
                    }

                    unsynced_count += 1;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Time limit hit, proceed to sync logic below
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // Main thread dead, one final sync and exit
                    let _ = writer.get_ref().sync_all();
                    break;
                }
            }

            // --- STEP B: CHECK IF WE NEED PHYSICAL DISK SYNC ---
            let should_sync = unsynced_count >= SYNC_BATCH_SIZE ||
                last_sync.elapsed() >= SYNC_TIMEOUT;

            if should_sync && unsynced_count > 0 {
                // The expensive call happens here, but much less frequently
                let _ = writer.get_ref().sync_all();

                // Reset counters
                unsynced_count = 0;
                last_sync = Instant::now();
            }
        }
    }

    fn flush_batch(writer: &mut BufWriter<File>, buffer: &mut Vec<LogEntry>, current_seq: &mut u64) {
        for entry in buffer.drain(..) {
            *current_seq += 1;
            let frame = WalFrame { seq: *current_seq, entry };

            // Serialize
            if let Ok(encoded) = bincode::serialize(&frame) {
                let len = encoded.len() as u64;
                // Write to OS buffer
                let _ = writer.write_all(&len.to_le_bytes());
                let _ = writer.write_all(&encoded);
            }
        }

        // CRITICAL: The physical disk sync happens ONCE for the whole batch
        let _ = writer.flush();
        let _ = writer.get_ref().sync_all(); // The expensive syscall
        // println!("   [Disk] Synced batch to disk. Seq: {}", current_seq);
    }

    // This is now non-blocking! It returns instantly.
    pub fn append(&mut self, entry: LogEntry) -> Result<()> {
        self.current_seq += 1;
        self.tx.send(entry).context("Failed to send to WAL thread")?;
        Ok(())
    }

    // Helper for recovery (same as before, runs on main thread)
    pub fn replay_from_disk(path: &Path) -> Result<(Vec<(u64, LogEntry)>, u64)> {
        if !path.exists() { return Ok((Vec::new(), 0)); }

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut entries = Vec::new();
        let mut max_seq = 0;

        loop {
            let mut len_buf = [0u8; 8];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {
                    let len = u64::from_le_bytes(len_buf) as usize;
                    let mut payload = vec![0u8; len];
                    reader.read_exact(&mut payload)?;
                    let frame: WalFrame = bincode::deserialize(&payload[..])?;

                    if frame.seq > max_seq { max_seq = frame.seq; }
                    entries.push((frame.seq, frame.entry));
                }
                Err(_) => break,
            }
        }
        Ok((entries, max_seq))
    }
}

// ==========================================
// 3. MATCHING ENGINE
// ==========================================

pub struct MatchingEngine {
    pub orders: HashMap<Ulid, Order>,
    pub wal: Wal,
    pub snapshot_dir: PathBuf,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        if !snapshot_dir.exists() { fs::create_dir_all(snapshot_dir)?; }

        let mut orders = HashMap::new();
        let mut recovered_seq = 0;

        // 1. Load Snapshot
        if let Some((_, path)) = Self::find_latest_snapshot(snapshot_dir)? {
            println!("   [Recover] Loading Snapshot: {:?}", path);
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            let snap: Snapshot = bincode::deserialize_from(reader)?;
            orders = snap.orders;
            recovered_seq = snap.last_seq;
        }

        // 2. Replay WAL to catch up
        println!("   [Recover] Replaying WAL...");
        let (history, file_seq) = Wal::replay_from_disk(wal_path)?;

        for (seq, entry) in history {
            if seq > recovered_seq {
                match entry {
                    LogEntry::PlaceOrder(o) => { orders.insert(o.id, o); }
                    LogEntry::CancelOrder { id } => { orders.remove(&id); }
                }
            }
        }

        let start_seq = std::cmp::max(recovered_seq, file_seq);
        println!("   [Recover] Complete. Starting Engine at Seq: {}", start_seq);

        // 3. Start Async WAL
        let wal = Wal::open(wal_path, start_seq)?;

        Ok(Self { orders, wal, snapshot_dir: snapshot_dir.to_path_buf() })
    }

    fn find_latest_snapshot(dir: &Path) -> Result<Option<(u64, PathBuf)>> {
        let mut max_seq = 0;
        let mut found_path = None;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
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

    pub fn place_order(&mut self, order: Order) -> Result<()> {
        // Send to background thread (Instant)
        self.wal.append(LogEntry::PlaceOrder(order.clone()))?;
        // Update Memory (Instant)
        self.orders.insert(order.id, order);
        Ok(())
    }
}

// ==========================================
// 4. MAIN (SPEED TEST)
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("async.wal");
    let snap_dir = Path::new("async_snaps");

    if wal_path.exists() { fs::remove_file(wal_path)?; }
    if snap_dir.exists() { fs::remove_dir_all(snap_dir)?; }

    let total_orders = 100_0000; // Increased to 100k to show off speed!

    println!(">>> STARTING ASYNC PERFOMANCE TEST ({} Orders)", total_orders);

    let mut engine = MatchingEngine::new(wal_path, snap_dir)?;
    let start = Instant::now();

    for _ in 0..total_orders {
        engine.place_order(Order {
            id: Ulid::new(),
            symbol: "BTC".into(),
            side: OrderSide::Buy,
            price: 100,
            quantity: 1,
        })?;
    }

    let duration = start.elapsed();
    println!("\n>>> RESULTS:");
    println!("    Total Time:    {:.2?}", duration);
    println!("    Orders/Sec:    {:.0} orders/sec", total_orders as f64 / duration.as_secs_f64());

    // Allow background thread to finish flushing before exiting
    // In a real app, you'd have a Proper shutdown signal
    println!("    (Waiting 20ms for background flush to finish...)");
    thread::sleep(Duration::from_millis(20));

    Ok(())
}