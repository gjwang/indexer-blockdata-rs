use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

// <--- Import ULID

// --- 1. Domain Types ---

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Order {
    pub id: Ulid, // <--- Changed from u64 to Ulid
    pub symbol: String,
    pub side: OrderSide,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum LogEntry {
    PlaceOrder(Order),
    CancelOrder { id: Ulid }, // <--- Changed here too
}

// --- 2. Write-Ahead Log (Unchanged Logic) ---

pub struct Wal {
    file: File,
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

        let writer_file = file.try_clone()?;
        Ok(Self {
            file,
            writer: BufWriter::new(writer_file),
        })
    }

    pub fn append(&mut self, entry: &LogEntry) -> Result<()> {
        let encoded: Vec<u8> = bincode::serialize(entry)?;
        let len = encoded.len() as u64;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&encoded[..])?; // Ensure slice usage
        self.writer.flush()?;
        self.file.sync_all()?;
        Ok(())
    }

    pub fn replay(&self) -> Result<Vec<LogEntry>> {
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
                    reader.read_exact(&mut payload[..])?;

                    // The magic happens here: Bincode automatically handles ULID binary format
                    let entry: LogEntry = bincode::deserialize(&payload[..])?;
                    entries.push(entry);
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(entries)
    }
}

// --- 3. Matching Engine ---

pub struct MatchingEngine {
    // Key is now Ulid, not u64
    orders: HashMap<Ulid, Order>,
    wal: Wal,
}

impl MatchingEngine {
    pub fn new(wal_path: &Path) -> Result<Self> {
        let wal = Wal::open(wal_path)?; // 'mut' removed as per previous fix

        let history = wal.replay()?;
        println!("Recovering... replayed {} events.", history.len());

        let mut engine = Self {
            orders: HashMap::new(),
            wal,
        };

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
        let id = order.id; // Return the ID so the caller knows it
        let entry = LogEntry::PlaceOrder(order);
        self.wal.append(&entry)?;
        self.apply_in_memory(&entry);
        Ok(id)
    }

    pub fn cancel_order(&mut self, id: Ulid) -> Result<()> {
        let entry = LogEntry::CancelOrder { id };
        self.wal.append(&entry)?;
        self.apply_in_memory(&entry);
        Ok(())
    }

    pub fn print_book(&self) {
        println!("--- Current Order Book ---");
        // We sort the output so we can see the time-ordering of ULIDs
        let mut sorted_orders: Vec<&Order> = self.orders.values().collect();

        // ULIDs sort lexicographically by time automatically!
        sorted_orders.sort_by_key(|o| o.id);

        for order in sorted_orders {
            println!("ID: {} | {:?} {} @ ${}", order.id, order.side, order.quantity, order.price);
        }
        println!("--------------------------");
    }
}

// --- 4. Demo Execution ---

fn main() -> Result<()> {
    let wal_path = Path::new("engine.wal");

    {
        println!(">>> Starting Engine Session 1");
        let mut engine = MatchingEngine::new(wal_path)?;

        // Generate new ULIDs for every order
        let id1 = Ulid::new();
        engine.place_order(Order { id: id1, symbol: "BTCUSDT".into(), side: OrderSide::Buy, price: 50000, quantity: 1 })?;

        // Simulate a tiny delay to ensure timestamps differ (optional, ULID handles same-ms too)
        // std::thread::sleep(std::time::Duration::from_millis(10));

        let id2 = Ulid::new();
        engine.place_order(Order { id: id2, symbol: "BTCUSDT".into(), side: OrderSide::Sell, price: 51000, quantity: 2 })?;

        engine.print_book();
        println!(">>> Engine Crash/Shutdown\n");
    }

    {
        println!(">>> Starting Engine Session 2 (Recovery)");
        let mut engine = MatchingEngine::new(wal_path)?;

        engine.print_book();

        println!(">>> Adding new order in Session 2");
        let id3 = Ulid::new();
        engine.place_order(Order { id: id3, symbol: "BTCUSDT".into(), side: OrderSide::Buy, price: 49000, quantity: 5 })?;
        engine.print_book();
    }

    Ok(())
}