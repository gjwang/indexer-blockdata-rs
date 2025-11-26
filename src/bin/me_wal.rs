use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

// --- 1. Domain Types ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Order {
    pub id: Ulid,
    pub symbol: String,
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
}

// The business logic payload
#[derive(Serialize, Deserialize, Debug)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid },
}

// --- 2. The Envelope (Wrapper) ---

// This acts as the "Transport Layer" for the WAL.
// The engine doesn't care about this, only the storage layer does.
#[derive(Serialize, Deserialize, Debug)]
struct WalFrame {
    pub seq: u64,        // Strictly increasing ID
    pub entry: LogEntry, // The actual data
}

// --- 3. Write-Ahead Log ---

pub struct Wal {
    file: File,
    writer: BufWriter<File>,
    pub current_seq: u64, // Tracks the last written ID
}

impl Wal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)
            .context("Failed to open WAL file")?;

        let writer_file = file.try_clone()?;

        Ok(Self {
            file,
            writer: BufWriter::new(writer_file),
            current_seq: 0, // Will be updated after replay
        })
    }

    pub fn append(&mut self, entry: LogEntry) -> Result<()> {
        // 1. Increment Sequence
        self.current_seq += 1;

        // 2. Wrap data in the Frame
        let frame = WalFrame {
            seq: self.current_seq,
            entry,
        };

        // 3. Serialize the FRAME (not just the entry)
        let encoded: Vec<u8> = bincode::serialize(&frame)?;
        let len = encoded.len() as u64;

        // 4. Write to disk
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&encoded[..])?;
        self.writer.flush()?;
        self.file.sync_all()?;

        Ok(())
    }

    pub fn replay(&mut self) -> Result<Vec<LogEntry>> {
        let mut file = &self.file;
        let mut entries = Vec::new();
        let mut reader = BufReader::new(file.try_clone()?);
        reader.seek(SeekFrom::Start(0))?;

        let mut expected_seq = 1;

        loop {
            let mut len_buf = [0u8; 8];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {
                    let len = u64::from_le_bytes(len_buf) as usize;
                    let mut payload = vec![0u8; len];
                    reader.read_exact(&mut payload[..])?;

                    // Deserialize the FRAME
                    let frame: WalFrame = bincode::deserialize(&payload[..])?;

                    // --- INTEGRITY CHECK ---
                    if frame.seq != expected_seq {
                        bail!(
                            "WAL CORRUPTION DETECTED! Expected Seq ID {}, but found {}. Possible missing data.",
                            expected_seq, frame.seq
                        );
                    }
                    // -----------------------

                    entries.push(frame.entry);

                    // Update internal state
                    self.current_seq = frame.seq;
                    expected_seq += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }

        Ok(entries)
    }
}

// --- 4. Matching Engine ---

pub struct MatchingEngine {
    orders: HashMap<Ulid, Order>,
    wal: Wal,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path) -> Result<Self> {
        // 1. Open WAL
        let mut wal = Wal::open(wal_path)?;

        // 2. Replay and Verify Sequences
        let history = wal.replay()?;

        println!("Recovering... replayed {} events. Last Seq ID: {}", history.len(), wal.current_seq);

        let mut engine = Self {
            orders: HashMap::new(),
            wal,
        };

        // 3. Rebuild Memory
        for entry in history {
            engine.apply_in_memory(&entry);
        }

        Ok(engine)
    }

    fn apply_in_memory(&mut self, entry: &LogEntry) {
        match entry {
            LogEntry::PlaceOrder(order) => {
                self.orders.insert(order.id, order.clone());
            }
            LogEntry::CancelOrder { id } => {
                self.orders.remove(id);
            }
        }
    }

    pub fn place_order(&mut self, order: Order) -> Result<Ulid> {
        let id = order.id;
        // WAL handles wrapping it in a frame and assigning Seq ID
        self.wal.append(LogEntry::PlaceOrder(order.clone()))?;
        self.apply_in_memory(&LogEntry::PlaceOrder(order));
        Ok(id)
    }

    pub fn print_book(&self) {
        println!("--- Order Book (Seq: {}) ---", self.wal.current_seq);
        let mut sorted_orders: Vec<&Order> = self.orders.values().collect();
        sorted_orders.sort_by_key(|o| o.id);
        for order in sorted_orders {
            println!("ID: {} | {:?} {} @ ${}", order.id, order.side, order.quantity, order.price);
        }
        println!("------------------------------");
    }
}

// --- 5. Demo ---

fn main() -> Result<()> {
    let wal_path = Path::new("engine_seq.wal");

    // Clean up previous runs for this demo
    // if wal_path.exists() { std::fs::remove_file(wal_path)?; }

    {
        println!(">>> Session 1");
        let mut engine = MatchingEngine::new(wal_path)?;

        engine.place_order(Order { id: Ulid::new(), symbol: "ETH".into(), side: OrderSide::Buy, price: 3000, quantity: 1 })?;
        engine.place_order(Order { id: Ulid::new(), symbol: "ETH".into(), side: OrderSide::Sell, price: 3100, quantity: 2 })?;
        engine.print_book();
    }

    {
        println!("\n>>> Session 2 (Recovery)");
        let mut engine = MatchingEngine::new(wal_path)?;

        println!("Adding a new order...");
        engine.place_order(Order { id: Ulid::new(), symbol: "ETH".into(), side: OrderSide::Buy, price: 2900, quantity: 5 })?;
        engine.print_book();
    }

    Ok(())
}