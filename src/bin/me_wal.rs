use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// --- 1. Domain Types (The "Trading" Data) ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Order {
    pub id: u64,
    pub symbol: String,
    pub side: OrderSide,
    pub price: u64, // Using u64 to avoid float errors in financial code
    pub quantity: u64,
}

// Events that change the engine state.
// We log *Inputs* (Commands), not just resulting trades, to ensure determinism.
#[derive(Serialize, Deserialize, Debug)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: u64 },
}

// --- 2. Write-Ahead Log (The Persistence Layer) ---

pub struct Wal {
    file: File, // Raw file handle for syncing
    writer: BufWriter<File>,
}

impl Wal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)
            .context("Failed to open WAL file")?;

        // Clone the file handle so we can have both a writer and a raw handle for fsync
        let writer_file = file.try_clone()?;

        Ok(Self {
            file,
            writer: BufWriter::new(writer_file),
        })
    }

    /// Appends an entry to the log and ensures it is physically written to disk.
    pub fn append(&mut self, entry: &LogEntry) -> Result<()> {
        // 1. Serialize the entry into binary
        let encoded: Vec<u8> = bincode::serialize(entry)?;

        // 2. Write the length prefix (u64) so we know how many bytes to read back later
        let len = encoded.len() as u64;
        self.writer.write_all(&len.to_le_bytes())?;

        // 3. Write the actual data
        self.writer.write_all(&encoded[..])?;
        // 4. Critical: Flush the buffer to the OS
        self.writer.flush()?;

        // 5. Critical: Sync to physical disk (fsync).
        // In extremely high-freq setups, this might be batched (e.g., every 5ms),
        // but for correctness, we do it per write here.
        self.file.sync_all()?;

        Ok(())
    }

    /// Reads all entries from the log to restore state.
    pub fn replay(&self) -> Result<Vec<LogEntry>> {
        let mut file = &self.file;
        let mut entries = Vec::new();

        // Ensure we start reading from the beginning
        let mut reader = BufReader::new(file.try_clone()?);
        reader.seek(SeekFrom::Start(0))?;

        loop {
            // 1. Read the length prefix (8 bytes)
            let mut len_buf = [0u8; 8];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {
                    let len = u64::from_le_bytes(len_buf) as usize;

                    // 2. Read the payload
                    let mut payload = vec![0u8; len];
                    reader.read_exact(&mut payload[..])?;

                    // 3. Deserialize
                    let entry: LogEntry = bincode::deserialize(&payload[..])?;
                    entries.push(entry);
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // End of log
                Err(e) => return Err(e.into()),
            }
        }

        Ok(entries)
    }
}

// --- 3. Matching Engine (The In-Memory State) ---

pub struct MatchingEngine {
    orders: HashMap<u64, Order>,
    wal: Wal,
}

impl MatchingEngine {
    // Initialize engine from a WAL path. If log exists, it recovers.
    pub fn new(wal_path: &Path) -> Result<Self> {
        let mut wal = Wal::open(wal_path)?;

        // RECOVERY STEP: Replay history to rebuild memory state
        let history = wal.replay()?;
        println!("Recovering... replayed {} events.", history.len());

        let mut engine = Self {
            orders: HashMap::new(),
            wal,
        };

        // Re-apply events to memory only (do not re-log them!)
        for entry in history {
            engine.apply_in_memory(&entry);
        }

        Ok(engine)
    }

    // Helper to apply logic to memory (used by both live processing and recovery)
    fn apply_in_memory(&mut self, entry: &LogEntry) {
        match entry {
            LogEntry::PlaceOrder(order) => {
                // In a real engine, you would check matching logic here
                self.orders.insert(order.id, order.clone());
            }
            LogEntry::CancelOrder { id } => {
                self.orders.remove(id);
            }
        }
    }

    // Public API: Handles the request, persists it, then updates memory
    pub fn place_order(&mut self, order: Order) -> Result<()> {
        let entry = LogEntry::PlaceOrder(order);

        // 1. Persist FIRST (WAL)
        self.wal.append(&entry)?;

        // 2. Update Memory SECOND
        self.apply_in_memory(&entry);

        Ok(())
    }

    pub fn cancel_order(&mut self, id: u64) -> Result<()> {
        let entry = LogEntry::CancelOrder { id };
        self.wal.append(&entry)?;
        self.apply_in_memory(&entry);
        Ok(())
    }

    pub fn print_book(&self) {
        println!("--- Current Order Book ---");
        for order in self.orders.values() {
            println!(
                "ID: {}, {:?} {} @ ${}",
                order.id, order.side, order.quantity, order.price
            );
        }
        println!("--------------------------");
    }
}

// --- 4. Demo Execution ---

fn main() -> Result<()> {
    let file_name = "engine.wal";
    let wal_path = Path::new(file_name);
    
    // Scoping to simulate a process running and then shutting down
    {
        println!(">>> Starting Engine Session 1");
        let mut engine = MatchingEngine::new(wal_path)?;

        engine.place_order(Order {
            id: 1,
            symbol: "BTCUSDT".into(),
            side: OrderSide::Buy,
            price: 50000,
            quantity: 1,
        })?;
        engine.place_order(Order {
            id: 2,
            symbol: "BTCUSDT".into(),
            side: OrderSide::Sell,
            price: 51000,
            quantity: 2,
        })?;
        engine.print_book();
        println!(">>> Engine Crash/Shutdown\n");
    }

    // New Scope: Simulate restart (memory is wiped, reading from disk)
    {
        println!(">>> Starting Engine Session 2 (Recovery)");
        let mut engine = MatchingEngine::new(wal_path)?;

        engine.print_book(); // Should show orders from Session 1

        println!(">>> Adding new order in Session 2");
        engine.place_order(Order {
            id: 3,
            symbol: "BTCUSDT".into(),
            side: OrderSide::Buy,
            price: 49000,
            quantity: 5,
        })?;
        engine.print_book();
    }

    Ok(())
}
