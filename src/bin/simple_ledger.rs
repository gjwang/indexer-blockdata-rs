use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use crc32fast::Hasher;
use memmap2::{MmapMut, MmapOptions};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// <--- NEW DEPENDENCY

// ==========================================
// 1. Data Structures (Same as before)
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
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            return &mut self.assets[index].1;
        }
        self.assets.push((asset, Balance { available: 0, frozen: 0 }));
        &mut self.assets.last_mut().unwrap().1
    }
}

// ==========================================
// 2. Commands (Same as before)
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
// 3. High Performance WAL (Mmap + CRC32)
// ==========================================

pub struct MmapWal {
    file: File,
    mmap: MmapMut,
    cursor: usize,
    len: usize,
}

impl MmapWal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .context("Failed to open WAL")?;

        let meta = file.metadata()?;
        let mut len = meta.len() as usize;

        // Pre-allocate 1GB for new files
        if len == 0 {
            len = 1024 * 1024 * 1024; // 1GB
            file.set_len(len as u64)?;
        }

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // --- SCAN & RECOVER CURSOR ---
        // Format: [Length u32] [CRC32 u32] [Payload...]
        let mut cursor = 0;
        while cursor + 8 < len {
            // 1. Read Length
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            // Zero length means end of written data
            if payload_len == 0 { break; }

            // Check if file is truncated/corrupt at end
            if cursor + 8 + payload_len > len {
                eprintln!("WARNING: WAL truncated at offset {}", cursor);
                break;
            }

            // 2. Read Stored CRC
            let crc_bytes: [u8; 4] = mmap[cursor + 4..cursor + 8].try_into().unwrap();
            let stored_crc = u32::from_le_bytes(crc_bytes);

            // 3. Verify CRC (Read Payload)
            let payload = &mmap[cursor + 8..cursor + 8 + payload_len];
            let mut hasher = Hasher::new();
            hasher.update(payload);
            let calculated_crc = hasher.finalize();

            if calculated_crc != stored_crc {
                bail!("CRITICAL: WAL Corruption detected at offset {}! Expected CRC {}, found {}. Possible disk failure.",
                      cursor, stored_crc, calculated_crc);
            }

            cursor += 8 + payload_len;
        }

        println!("WAL Opened. Verified Integrity. Resuming from offset: {}", cursor);

        Ok(Self { file, mmap, cursor, len })
    }

    #[inline(always)]
    pub fn append(&mut self, cmd: &LedgerCommand) -> Result<()> {
        // 1. Serialize to buffer (Stack allocation, fast)
        // In extremely high perf code, we might reuse a thread-local buffer
        let payload = bincode::serialize(cmd)?;
        let size = payload.len();

        // 2. Auto-Grow Logic
        // We need 4 bytes (Len) + 4 bytes (CRC) + Payload Size
        if self.cursor + 8 + size >= self.len {
            self.remap_grow()?;
        }

        // 3. Calculate CRC32 (Hardware Accelerated)
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let crc = hasher.finalize();

        // 4. Write Header [Length u32] [CRC u32]
        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&(size as u32).to_le_bytes());
        self.mmap[self.cursor + 4..self.cursor + 8].copy_from_slice(&crc.to_le_bytes());

        // 5. Write Payload
        self.mmap[self.cursor + 8..self.cursor + 8 + size].copy_from_slice(&payload);

        self.cursor += 8 + size;

        Ok(())
    }

    fn remap_grow(&mut self) -> Result<()> {
        self.mmap.flush()?;
        let new_len = self.len * 2;
        println!(">>> WAL Full. Extending from {} MB to {} MB...",
                 self.len / 1024 / 1024, new_len / 1024 / 1024);

        self.file.set_len(new_len as u64)?;
        self.mmap = unsafe { MmapOptions::new().map_mut(&self.file)? };
        self.len = new_len;
        Ok(())
    }

    // Recovery function: Returns valid commands
    pub fn replay(&self) -> Result<Vec<LedgerCommand>> {
        let mut commands = Vec::new();
        let mut cursor = 0;

        // We trust the structure because we verified CRCs in open()
        while cursor + 8 < self.len {
            let len_bytes: [u8; 4] = self.mmap[cursor..cursor + 4].try_into().unwrap();
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len == 0 { break; }

            // Skip Length (4) + CRC (4) to get Payload
            let payload = &self.mmap[cursor + 8..cursor + 8 + payload_len];

            let cmd: LedgerCommand = bincode::deserialize(payload)?;
            commands.push(cmd);

            cursor += 8 + payload_len;
        }
        Ok(commands)
    }
}

// ==========================================
// 4. Global Ledger (Logic)
// ==========================================

pub struct GlobalLedger {
    accounts: FxHashMap<UserId, UserAccount>,
    wal: MmapWal,
    seq: u64,
}

impl GlobalLedger {
    pub fn new(wal_path: &Path) -> Result<Self> {
        let wal = MmapWal::open(wal_path)?;

        let mut ledger = Self {
            accounts: FxHashMap::default(),
            wal,
            seq: 0,
        };

        println!("Replaying WAL...");
        let history = ledger.wal.replay()?;
        let count = history.len();

        for cmd in history {
            ledger.apply_logic(&cmd)?;
            ledger.seq += 1;
        }
        println!("Recovered {} transactions. Current Seq: {}", count, ledger.seq);

        Ok(ledger)
    }

    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        // 1. Persistence (With CRC)
        self.wal.append(cmd)?;
        self.seq += 1;

        // 2. Memory State
        self.apply_logic(cmd)?;

        Ok(())
    }

    // Logic separated for replay
    fn apply_logic(&mut self, cmd: &LedgerCommand) -> Result<()> {
        match cmd {
            LedgerCommand::Deposit { user_id, asset, amount } => {
                let user = self.get_user(*user_id);
                let bal = user.get_balance_mut(*asset);
                bal.available = bal.available.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }

            LedgerCommand::Withdraw { user_id, asset, amount } => {
                let user = self.get_or_error(*user_id)?;
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount { bail!("Insufficient funds"); }
                bal.available -= amount;
            }

            LedgerCommand::Lock { user_id, asset, amount } => {
                let user = self.get_or_error(*user_id)?;
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount { bail!("Insufficient available"); }
                bal.available -= amount;
                bal.frozen = bal.frozen.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }

            LedgerCommand::Unlock { user_id, asset, amount } => {
                let user = self.get_or_error(*user_id)?;
                let bal = user.get_balance_mut(*asset);
                if bal.frozen < *amount { bail!("Insufficient frozen"); }
                bal.frozen -= amount;
                bal.available = bal.available.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }

            LedgerCommand::TradeSettle { user_id, spend_asset, spend_amount, gain_asset, gain_amount } => {
                let user = self.get_or_error(*user_id)?;
                let spend_bal = user.get_balance_mut(*spend_asset);
                if spend_bal.frozen < *spend_amount { bail!("CRITICAL: Trade spend > frozen"); }
                spend_bal.frozen -= spend_amount;

                let gain_bal = user.get_balance_mut(*gain_asset);
                gain_bal.available = gain_bal.available.checked_add(*gain_amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn get_user(&mut self, user_id: UserId) -> &mut UserAccount {
        self.accounts.entry(user_id).or_insert_with(|| UserAccount::new(user_id))
    }

    #[inline(always)]
    fn get_or_error(&mut self, user_id: UserId) -> Result<&mut UserAccount> {
        self.accounts.get_mut(&user_id).ok_or(anyhow::anyhow!("User not found"))
    }
}

// ==========================================
// 5. Main (With Benchmark)
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("ledger_crc.wal");
    if wal_path.exists() { fs::remove_file(wal_path)?; }

    let mut ledger = GlobalLedger::new(wal_path)?;

    // --- Phase 1: Pre-warm ---
    let user_count = 10_000;
    println!(">>> SESSION 1: Pre-warming data ({} users)...", user_count);
    for id in 0..user_count {
        ledger.apply(&LedgerCommand::Deposit {
            user_id: id as u64,
            asset: 1,
            amount: 1_000_000,
        })?;
    }
    println!("    Pre-warm complete.");

    // --- Phase 2: Performance Benchmark ---
    println!("\n>>> SESSION 2: Performance Benchmark (WAL + CRC32)");
    println!("    Scenario: 2M Ordered Operations");

    let total_ops = 2_000_000;
    let start = Instant::now();

    for i in 0..total_ops {
        let user_id = (i % user_count) as u64;
        let round = i / user_count;

        match round % 3 {
            0 => { // Deposit
                ledger.apply(&LedgerCommand::Deposit { user_id, asset: 1, amount: 100 })?;
            }
            1 => { // Lock
                ledger.apply(&LedgerCommand::Lock { user_id, asset: 1, amount: 50 })?;
            }
            _ => { // Unlock
                ledger.apply(&LedgerCommand::Unlock { user_id, asset: 1, amount: 50 })?;
            }
        }
    }

    let duration = start.elapsed();
    let seconds = duration.as_secs_f64();
    let tps = total_ops as f64 / seconds;

    println!("--------------------------------------------------");
    println!("    Total Ops:    {}", total_ops);
    println!("    Total Time:   {:.4} s", seconds);
    println!("    Throughput:   {:.0} ops/sec", tps);
    println!("    Avg Latency:  {:.2?} / op", duration / total_ops as u32);
    println!("--------------------------------------------------");

    // --- Phase 3: Integrity Check Demo ---
    // In a real test, you could manually corrupt a byte in the file here
    // and assert that re-opening it causes a Panic/Error.

    Ok(())
}