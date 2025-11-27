use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use crc32fast::Hasher;
use memmap2::{MmapMut, MmapOptions};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// ==========================================
// 1. Configuration Constants
// ==========================================

// Safety Limit: Max 10MB per record.
// This prevents OOM crashes if a corrupted length byte claims a huge size.
const MAX_RECORD_SIZE: usize = 10 * 1024 * 1024;

// Read Buffer Size: 1MB.
// Significantly reduces system calls during recovery.
const READ_BUFFER_SIZE: usize = 1024 * 1024;

// ==========================================
// 2. Data Structures
// ==========================================

pub type AssetId = u32;
pub type UserId = u64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[repr(C)]
pub struct Balance {
    pub available: u64,
    pub frozen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    pub user_id: UserId,
    pub assets: Vec<(AssetId, Balance)>,
}

impl UserAccount {
    pub fn new(user_id: UserId) -> Self {
        Self { user_id, assets: Vec::with_capacity(8) }
    }

    #[inline(always)]
    pub fn get_balance_mut(&mut self, asset: AssetId) -> &mut Balance {
        // Optimization: Linear scan on Vec is faster than HashMap for small N (<20 items)
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            return &mut self.assets[index].1;
        }
        self.assets.push((asset, Balance { available: 0, frozen: 0 }));
        &mut self.assets.last_mut().unwrap().1
    }
}

// ==========================================
// 3. Commands
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LedgerCommand {
    Deposit { user_id: UserId, asset: AssetId, amount: u64 },
    Withdraw { user_id: UserId, asset: AssetId, amount: u64 },
    Lock { user_id: UserId, asset: AssetId, amount: u64 },
    Unlock { user_id: UserId, asset: AssetId, amount: u64 },
    TradeSettle {
        user_id: UserId,
        spend_asset: AssetId,
        spend_amount: u64,
        gain_asset: AssetId,
        gain_amount: u64,
    },
}

// ==========================================
// 4. Streaming WAL Iterator (The Reader)
// ==========================================

pub struct WalIterator {
    reader: BufReader<File>,
    cursor: u64, // Track offset for error reporting
}

impl WalIterator {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        // Optimization: Use a large 1MB buffer
        let mut reader = BufReader::with_capacity(READ_BUFFER_SIZE, file);
        reader.seek(SeekFrom::Start(0))?;

        Ok(Self { reader, cursor: 0 })
    }
}

impl Iterator for WalIterator {
    type Item = Result<LedgerCommand>;

    fn next(&mut self) -> Option<Self::Item> {
        // 1. Read Length (4 bytes)
        let mut len_buf = [0u8; 4];
        match self.reader.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None, // EOF
            Err(e) => return Some(Err(e.into())),
        }

        let payload_len = u32::from_le_bytes(len_buf) as usize;

        // [Safety Check] Prevent reading huge garbage data
        if payload_len > MAX_RECORD_SIZE {
            return Some(Err(anyhow::anyhow!(
                "CRITICAL: Record length {} exceeds limit {} at offset {}",
                payload_len, MAX_RECORD_SIZE, self.cursor
            )));
        }

        // Zero padding usually indicates end of valid data in pre-allocated files
        if payload_len == 0 { return None; }

        // 2. Read CRC (4 bytes)
        let mut crc_buf = [0u8; 4];
        if let Err(e) = self.reader.read_exact(&mut crc_buf) {
            return Some(Err(e.into()));
        }
        let stored_crc = u32::from_le_bytes(crc_buf);

        // 3. Read Payload
        // Allocating a Vec here is cheap because payload_len is usually small (e.g., <100 bytes)
        let mut payload = vec![0u8; payload_len];
        if let Err(e) = self.reader.read_exact(&mut payload) {
            return Some(Err(e.into()));
        }

        // 4. [Integrity Check] Verify CRC
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let calc_crc = hasher.finalize();

        if calc_crc != stored_crc {
            return Some(Err(anyhow::anyhow!(
                "CRITICAL: CRC Mismatch at offset {}! Stored: {}, Calc: {}",
                self.cursor, stored_crc, calc_crc
            )));
        }

        // 5. Deserialize
        let cmd = bincode::deserialize(&payload).map_err(|e| e.into());

        // Update cursor tracker
        self.cursor += 8 + payload_len as u64;
        Some(cmd)
    }
}

// ==========================================
// 5. Hybrid WAL (Writer: Mmap, Reader: Iterator)
// ==========================================

pub struct HybridWal {
    file: File,
    mmap: MmapMut,
    cursor: usize,
    len: usize,
    path: PathBuf, // Stored for creating iterators
}

impl HybridWal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
        let meta = file.metadata()?;
        let mut len = meta.len() as usize;

        // Pre-allocate 1GB for new files
        if len == 0 {
            len = 1024 * 1024 * 1024;
            file.set_len(len as u64)?;
        }

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Fast Scan: Find write cursor by skipping headers
        // We do NOT read payload or verify CRC here (that's for Replay)
        let mut cursor = 0;
        while cursor + 8 < len {
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len == 0 { break; }
            // Bounds check
            if cursor + 8 + payload_len > len { break; }

            cursor += 8 + payload_len;
        }

        println!("   [WAL] Write cursor at: {}", cursor);
        Ok(Self { file, mmap, cursor, len, path: path.to_path_buf() })
    }

    // Returns the high-performance streaming iterator
    pub fn iter(&self) -> Result<WalIterator> {
        WalIterator::new(&self.path)
    }

    #[inline(always)]
    pub fn append(&mut self, cmd: &LedgerCommand) -> Result<()> {
        let payload = bincode::serialize(cmd)?;
        let size = payload.len();

        // [Safety Check] Ensure we don't write huge records
        if size > MAX_RECORD_SIZE {
            bail!("Record size {} exceeds MAX_RECORD_SIZE {}", size, MAX_RECORD_SIZE);
        }

        // Auto-Grow logic
        if self.cursor + 8 + size >= self.len {
            self.remap_grow()?;
        }

        // Calculate CRC
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let crc = hasher.finalize();

        // Write: [Length 4B] [CRC 4B] [Payload...]
        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&(size as u32).to_le_bytes());
        self.mmap[self.cursor + 4..self.cursor + 8].copy_from_slice(&crc.to_le_bytes());
        self.mmap[self.cursor + 8..self.cursor + 8 + size].copy_from_slice(&payload);

        self.cursor += 8 + size;
        Ok(())
    }

    fn remap_grow(&mut self) -> Result<()> {
        self.mmap.flush()?;
        let new_len = self.len * 2;
        self.file.set_len(new_len as u64)?;
        self.mmap = unsafe { MmapOptions::new().map_mut(&self.file)? };
        self.len = new_len;
        Ok(())
    }
}

// ==========================================
// 6. Global Ledger Logic
// ==========================================

pub struct GlobalLedger {
    accounts: FxHashMap<UserId, UserAccount>,
    wal: HybridWal,
}

impl GlobalLedger {
    pub fn new(wal_path: &Path) -> Result<Self> {
        // 1. Separate initialization to satisfy Borrow Checker
        let wal = HybridWal::open(wal_path)?;
        let mut accounts = FxHashMap::default();

        println!("   [System] Replaying WAL (Streaming)...");
        let start = Instant::now();
        let mut count = 0;

        // 2. Replay using 1MB BufReader
        for cmd_result in wal.iter()? {
            let cmd = cmd_result?; // Will return Err if CRC fail
            Self::apply_transaction(&mut accounts, &cmd)?;
            count += 1;

            // Print progress every 10k records
            if count % 10_000 == 0 {
                // \r moves cursor to start of line, creating an updating effect
                print!("   [Recover] Processed {:>10} transactions...\r", count);
                std::io::stdout().flush().unwrap();
            }
        }

        println!("   [System] Replayed {} txs in {:.2?}", count, start.elapsed());

        // 3. Assemble
        Ok(Self { accounts, wal })
    }

    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        // 1. Write WAL (with safety checks)
        self.wal.append(cmd)?;
        // 2. Update Memory
        Self::apply_transaction(&mut self.accounts, cmd)
    }

    // Static logic function to decouple 'self' borrow
    fn apply_transaction(accounts: &mut FxHashMap<UserId, UserAccount>, cmd: &LedgerCommand) -> Result<()> {
        match cmd {
            LedgerCommand::Deposit { user_id, asset, amount } => {
                let user = accounts.entry(*user_id).or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset);
                bal.available = bal.available.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }
            LedgerCommand::Withdraw { user_id, asset, amount } => {
                let user = accounts.get_mut(user_id).context("User not found")?;
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount { bail!("Insufficient funds"); }
                bal.available -= amount;
            }
            LedgerCommand::Lock { user_id, asset, amount } => {
                let user = accounts.get_mut(user_id).context("User not found")?;
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount { bail!("Insufficient available"); }
                bal.available -= amount;
                bal.frozen = bal.frozen.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }
            LedgerCommand::Unlock { user_id, asset, amount } => {
                let user = accounts.get_mut(user_id).context("User not found")?;
                let bal = user.get_balance_mut(*asset);
                if bal.frozen < *amount { bail!("Insufficient frozen"); }
                bal.frozen -= amount;
                bal.available = bal.available.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }
            LedgerCommand::TradeSettle { user_id, spend_asset, spend_amount, gain_asset, gain_amount } => {
                let user = accounts.get_mut(user_id).context("User not found")?;
                let spend_bal = user.get_balance_mut(*spend_asset);
                if spend_bal.frozen < *spend_amount { bail!("CRITICAL: Trade spend > frozen"); }
                spend_bal.frozen -= spend_amount;

                let gain_bal = user.get_balance_mut(*gain_asset);
                gain_bal.available = gain_bal.available.checked_add(*gain_amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }
        }
        Ok(())
    }
}

// ==========================================
// 7. Main (Performance Test)
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("safe_wal.log");

    // Clean environment for accurate benchmark
    if wal_path.exists() { fs::remove_file(wal_path)?; }

    let mut ledger = GlobalLedger::new(wal_path)?;

    // --- Phase 1: Pre-warm ---
    let user_count = 10_000;
    println!(">>> SESSION 1: Pre-warming data...");
    for id in 0..user_count {
        ledger.apply(&LedgerCommand::Deposit { user_id: id as u64, asset: 1, amount: 1_000_000 })?;
    }

    // --- Phase 2: High Ops Benchmark ---
    println!("\n>>> SESSION 2: Performance Benchmark (Write)");
    println!("    Scenario: 2M Ordered Operations (Deposit -> Lock -> Unlock)");

    let total_ops = 20_000_000;
    let start = Instant::now();

    for i in 0..total_ops {
        let round = i / user_count;
        let user_id = (i % user_count) as u64;

        match round % 3 {
            0 => ledger.apply(&LedgerCommand::Deposit { user_id, asset: 1, amount: 100 })?,
            1 => ledger.apply(&LedgerCommand::Lock { user_id, asset: 1, amount: 50 })?,
            _ => ledger.apply(&LedgerCommand::Unlock { user_id, asset: 1, amount: 50 })?,
        };
    }

    let duration = start.elapsed();
    println!("    Total Ops:    {}", total_ops);
    println!("    Time:         {:.4} s", duration.as_secs_f64());
    println!("    Throughput:   {:.0} ops/sec", total_ops as f64 / duration.as_secs_f64());

    // --- Phase 3: Replay Test (BufReader Stream) ---
    println!("\n>>> SESSION 3: Streaming Replay (1MB Buffer)");
    drop(ledger); // Close file handles

    let start_replay = Instant::now();
    let _recovered = GlobalLedger::new(wal_path)?;
    let replay_dur = start_replay.elapsed();

    println!("    Replay Time:  {:.2?}", replay_dur);
    // Calculation includes pre-warm data
    println!("    Replay Speed: {:.0} tx/sec", (total_ops + user_count) as f64 / replay_dur.as_secs_f64());

    Ok(())
}