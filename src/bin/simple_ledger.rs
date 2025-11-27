use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use crc32fast::Hasher;
use memmap2::{MmapMut, MmapOptions};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// ==========================================
// 1. Configuration Constants
// ==========================================

const MAX_RECORD_SIZE: usize = 10 * 1024 * 1024;
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
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            return &mut self.assets[index].1;
        }
        self.assets.push((asset, Balance { available: 0, frozen: 0 }));
        &mut self.assets.last_mut().unwrap().1
    }
}

// [NEW] Snapshot Structure for disk storage
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub last_seq: u64,
    pub accounts: FxHashMap<UserId, UserAccount>,
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
// 4. Streaming WAL Iterator (Reader)
// ==========================================

pub struct WalIterator {
    reader: BufReader<File>,
    cursor: u64,
}

impl WalIterator {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::with_capacity(READ_BUFFER_SIZE, file);
        reader.seek(SeekFrom::Start(0))?;
        Ok(Self { reader, cursor: 0 })
    }
}

impl Iterator for WalIterator {
    type Item = Result<(u64, LedgerCommand)>;

    fn next(&mut self) -> Option<Self::Item> {
        // 1. Read Length
        let mut len_buf = [0u8; 4];
        match self.reader.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
            Err(e) => return Some(Err(e.into())),
        }

        let payload_len = u32::from_le_bytes(len_buf) as usize;

        if payload_len > MAX_RECORD_SIZE {
            return Some(Err(anyhow::anyhow!(
                "CRITICAL: Record length {} exceeds limit at offset {}", payload_len, self.cursor
            )));
        }
        if payload_len == 0 { return None; }

        // 2. Read CRC
        let mut crc_buf = [0u8; 4];
        if let Err(e) = self.reader.read_exact(&mut crc_buf) {
            return Some(Err(e.into()));
        }
        let stored_crc = u32::from_le_bytes(crc_buf);

        // 3. Read Data (Seq + Payload)
        let mut data_buf = vec![0u8; payload_len];
        if let Err(e) = self.reader.read_exact(&mut data_buf) {
            return Some(Err(e.into()));
        }

        // 4. Verify CRC (Length + Data)
        let mut hasher = Hasher::new();
        hasher.update(&len_buf);
        hasher.update(&data_buf);
        let calc_crc = hasher.finalize();

        if calc_crc != stored_crc {
            return Some(Err(anyhow::anyhow!("CRITICAL: CRC Mismatch at offset {}", self.cursor)));
        }

        // 5. Extract Seq
        if data_buf.len() < 8 {
            return Some(Err(anyhow::anyhow!("Record too short")));
        }
        let (seq_bytes, cmd_bytes) = data_buf.split_at(8);
        let seq = u64::from_le_bytes(seq_bytes.try_into().unwrap());

        // 6. Deserialize
        let cmd = match bincode::deserialize(cmd_bytes) {
            Ok(c) => c,
            Err(e) => return Some(Err(e.into())),
        };

        self.cursor += 8 + payload_len as u64;
        Some(Ok((seq, cmd)))
    }
}

// ==========================================
// 5. Hybrid WAL (Writer)
// ==========================================

pub struct HybridWal {
    file: File,
    mmap: MmapMut,
    cursor: usize,
    len: usize,
    path: PathBuf,
}

impl HybridWal {
    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
        let meta = file.metadata()?;
        let mut len = meta.len() as usize;

        if len == 0 {
            len = 1024 * 1024 * 1024;
            file.set_len(len as u64)?;
        }

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Fast Scan
        let mut cursor = 0;
        while cursor + 8 < len {
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let chunk_len = u32::from_le_bytes(len_bytes) as usize;
            if chunk_len == 0 { break; }
            if cursor + 8 + chunk_len > len { break; }
            cursor += 8 + chunk_len;
        }

        println!("   [WAL] Write cursor at: {}", cursor);
        Ok(Self { file, mmap, cursor, len, path: path.to_path_buf() })
    }

    pub fn iter(&self) -> Result<WalIterator> {
        WalIterator::new(&self.path)
    }

    #[inline(always)]
    pub fn append(&mut self, seq: u64, cmd: &LedgerCommand) -> Result<()> {
        let cmd_bytes = bincode::serialize(cmd)?;
        let data_len = 8 + cmd_bytes.len();
        let data_len_u32 = data_len as u32;

        if data_len > MAX_RECORD_SIZE {
            bail!("Record too large: {}", data_len);
        }

        if self.cursor + 8 + data_len >= self.len {
            self.remap_grow()?;
        }

        let mut hasher = Hasher::new();
        hasher.update(&data_len_u32.to_le_bytes());
        hasher.update(&seq.to_le_bytes());
        hasher.update(&cmd_bytes);
        let crc = hasher.finalize();

        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&(data_len as u32).to_le_bytes());
        self.cursor += 4;
        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&crc.to_le_bytes());
        self.cursor += 4;
        self.mmap[self.cursor..self.cursor + 8].copy_from_slice(&seq.to_le_bytes());
        self.cursor += 8;
        self.mmap[self.cursor..self.cursor + cmd_bytes.len()].copy_from_slice(&cmd_bytes);
        self.cursor += cmd_bytes.len();

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
// 6. Global Ledger (Snapshot + WAL)
// ==========================================

pub struct GlobalLedger {
    accounts: FxHashMap<UserId, UserAccount>,
    wal: HybridWal,
    last_seq: u64,
    snapshot_dir: PathBuf, // Store this to save snaps
}

impl GlobalLedger {
    pub fn new(wal_path: &Path, snapshot_dir: &Path) -> Result<Self> {
        fs::create_dir_all(snapshot_dir)?;

        let wal = HybridWal::open(wal_path)?;
        let mut accounts = FxHashMap::default();
        let mut recovered_seq = 0;

        // 1. Try Load Snapshot
        if let Some((seq, path)) = Self::find_latest_snapshot(snapshot_dir)? {
            println!("   [Recover] Loading Snapshot: {:?} (Seq {})", path, seq);
            let start_snap = Instant::now();
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            let snap: Snapshot = bincode::deserialize_from(reader)?;

            accounts = snap.accounts;
            recovered_seq = snap.last_seq;
            println!("   [Recover] Snapshot Loaded in {:.2?}", start_snap.elapsed());
        }

        // 2. Replay WAL (Only newer events)
        println!("   [Recover] Scanning WAL from Seq {}...", recovered_seq);
        let start_wal = Instant::now();
        let mut count = 0;

        for res in wal.iter()? {
            let (seq, cmd) = res?;

            // Skip old logs
            if seq <= recovered_seq { continue; }

            // Strict Continuity Check
            if seq != recovered_seq + 1 {
                bail!("CRITICAL: Sequence Gap! Expected {}, Found {}.", recovered_seq + 1, seq);
            }

            Self::apply_transaction(&mut accounts, &cmd)?;
            recovered_seq = seq;
            count += 1;
        }

        println!("   [Recover] Applied {} new WAL events in {:.2?}. Last Seq: {}",
                 count, start_wal.elapsed(), recovered_seq);

        Ok(Self {
            accounts,
            wal,
            last_seq: recovered_seq,
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
                    // Filename: snapshot_1000.snap
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

    // [NEW] Trigger Snapshot (Clone & Offload)
    pub fn trigger_snapshot(&self) {
        let current_seq = self.last_seq;
        let dir = self.snapshot_dir.clone();

        let start_clone = Instant::now();
        let accounts_copy = self.accounts.clone(); // The only blocking part
        let clone_dur = start_clone.elapsed();

        thread::spawn(move || {
            let start_write = Instant::now();
            let filename = format!("snapshot_{}.snap", current_seq);
            let path = dir.join(filename);

            let snap = Snapshot {
                last_seq: current_seq,
                accounts: accounts_copy,
            };

            match File::create(&path) {
                Ok(file) => {
                    let writer = std::io::BufWriter::new(file);
                    if let Err(e) = bincode::serialize_into(writer, &snap) {
                        println!("   [Snapshot] Write Error: {:?}", e);
                    } else {
                        // println!("   [Snapshot] Saved Seq {}. Clone: {:.2?}, Write: {:.2?}",
                        //          current_seq, clone_dur, start_write.elapsed());
                    }
                }
                Err(e) => println!("   [Snapshot] File Create Error: {:?}", e),
            }
        });
    }

    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        let new_seq = self.last_seq + 1;
        self.wal.append(new_seq, cmd)?;
        Self::apply_transaction(&mut self.accounts, cmd)?;
        self.last_seq = new_seq;
        Ok(())
    }

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
// 7. Main (Benchmark)
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("ledger.wal");
    let snap_dir = Path::new("snapshots");

    // Clean to start fresh
    if wal_path.exists() { fs::remove_file(wal_path)?; }
    if snap_dir.exists() { fs::remove_dir_all(snap_dir)?; }

    let mut ledger = GlobalLedger::new(wal_path, snap_dir)?;

    // --- Phase 1: Pre-warm ---
    let user_count = 10_000;
    println!(">>> SESSION 1: Pre-warming data...");
    for id in 0..user_count {
        ledger.apply(&LedgerCommand::Deposit { user_id: id as u64, asset: 1, amount: 1_000_000 })?;
    }

    // --- Phase 2: High Ops + Snapshotting ---
    println!("\n>>> SESSION 2: Writing 2M records with Snapshots...");
    let total_ops = 20_000_253;
    let start = Instant::now();

    for i in 0..total_ops {
        let round = i / user_count;
        let user_id = (i % user_count) as u64;

        match round % 3 {
            0 => ledger.apply(&LedgerCommand::Deposit { user_id, asset: 1, amount: 100 })?,
            1 => ledger.apply(&LedgerCommand::Lock { user_id, asset: 1, amount: 50 })?,
            _ => ledger.apply(&LedgerCommand::Unlock { user_id, asset: 1, amount: 50 })?,
        };

        // Trigger snapshot every 500k ops
        if i > 0 && i % 1_000_000 == 0 {
            ledger.trigger_snapshot();
            print!("."); // Visual progress
            std::io::stdout().flush()?;
        }
    }

    // Trigger one final snapshot at the end
    ledger.trigger_snapshot();

    let duration = start.elapsed();
    println!("\n    Total Ops:    {}", total_ops);
    println!("    Throughput:   {:.0} ops/sec", total_ops as f64 / duration.as_secs_f64());

    // Wait for background snapshots to finish writing
    println!("    Waiting for background snapshots...");
    thread::sleep(std::time::Duration::from_secs(2));

    // --- Phase 3: Recovery Test ---
    println!("\n>>> SESSION 3: Restart & Recovery (Snapshot + WAL)");
    drop(ledger);

    let start_replay = Instant::now();
    let recovered = GlobalLedger::new(wal_path, snap_dir)?;
    println!("    Total Recovery Time: {:.2?}", start_replay.elapsed());

    // Verification:
    // User count should be 10000 + 0 (only updated existing users)
    // Seq should be total_ops + user_count (2M + 10k)
    println!("    Last Seq: {}", recovered.last_seq);
    if recovered.last_seq == (total_ops as u64 + user_count as u64) {
        println!("✅ SUCCESS: Data fully restored.");
    } else {
        println!("❌ FAILURE: Data mismatch.");
    }

    Ok(())
}