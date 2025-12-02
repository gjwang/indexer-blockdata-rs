use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Result};
use crc32fast::Hasher;
use memmap2::{MmapMut, MmapOptions};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// =================================================================
// 0. Module Import
// =================================================================
use crate::md5_utils::{Md5Reader, Md5Writer};
// ==========================================
// 1. Configuration Constants
// ==========================================

const MAX_RECORD_SIZE: usize = 10 * 1024 * 1024;
const READ_BUFFER_SIZE: usize = 1024 * 1024;

// WAL Configuration
const WAL_MAX_SIZE: u64 = 512 * 1024 * 1024; // 1MB for demo rolling
const WAL_ROLL_TIME: Duration = Duration::from_secs(5 * 60);
const WAL_RETENTION: usize = 3;

// Snapshot Configuration
const SNAPSHOT_RETENTION: usize = 3;

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
        Self {
            user_id,
            assets: Vec::with_capacity(8),
        }
    }
    #[inline(always)]
    pub fn get_balance_mut(&mut self, asset: AssetId) -> &mut Balance {
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            return &mut self.assets[index].1;
        }
        self.assets.push((
            asset,
            Balance {
                available: 0,
                frozen: 0,
            },
        ));
        &mut self.assets.last_mut().unwrap().1
    }
}

// Snapshot on disk
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub last_seq: u64,
    pub accounts: FxHashMap<UserId, UserAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LedgerCommand {
    Deposit {
        user_id: UserId,
        asset: AssetId,
        amount: u64,
    },
    Withdraw {
        user_id: UserId,
        asset: AssetId,
        amount: u64,
    },
    Lock {
        user_id: UserId,
        asset: AssetId,
        amount: u64,
    },
    Unlock {
        user_id: UserId,
        asset: AssetId,
        amount: u64,
    },
    TradeSettle {
        user_id: UserId,
        spend_asset: AssetId,
        spend_amount: u64,
        gain_asset: AssetId,
        gain_amount: u64,
    },
    MatchExec(MatchExecData),
    MatchExecBatch(Vec<MatchExecData>),
    Batch(Vec<LedgerCommand>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchExecData {
    pub trade_id: u64,
    pub buyer_user_id: UserId,
    pub seller_user_id: UserId,
    pub price: u64,
    pub quantity: u64,
    pub base_asset: AssetId,
    pub quote_asset: AssetId,
    pub buyer_refund: u64,
    pub seller_refund: u64,
}

// ==========================================
// 3. Single WAL Segment (With Incremental MD5)
// ==========================================

pub struct WalSegment {
    mmap: MmapMut,
    cursor: usize,
    len: usize,
    md5_ctx: md5::Context,
    pub current_path: PathBuf,
}

impl WalSegment {
    pub fn create(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.set_len(WAL_MAX_SIZE)?;
        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        Ok(Self {
            mmap,
            cursor: 0,
            len: WAL_MAX_SIZE as usize,
            md5_ctx: md5::Context::new(),
            current_path: path.to_path_buf(),
        })
    }

    pub fn open_existing(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Re-calculate MD5 for existing data
        let mut cursor = 0;
        let mut md5_ctx = md5::Context::new();

        while cursor + 8 < len {
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len == 0 {
                break;
            }
            if cursor + 8 + payload_len > len {
                break;
            }

            // Hash the entire record (Len + CRC + Seq + Payload)
            let total_len = 8 + 8 + payload_len;
            let data_slice = &mmap[cursor..cursor + total_len];
            md5_ctx.consume(data_slice);

            cursor += total_len;
        }

        Ok(Self {
            mmap,
            cursor,
            len,
            md5_ctx,
            current_path: path.to_path_buf(),
        })
    }

    #[inline(always)]
    pub fn append(&mut self, seq: u64, cmd: &LedgerCommand) -> Result<()> {
        let cmd_bytes = bincode::serialize(cmd)?;
        let data_len = 8 + cmd_bytes.len();

        if self.cursor + 8 + data_len > self.len {
            bail!("Segment Full");
        }

        let mut hasher = Hasher::new();
        hasher.update(&(data_len as u32).to_le_bytes());
        hasher.update(&seq.to_le_bytes());
        hasher.update(&cmd_bytes);
        let crc = hasher.finalize();

        let len_bytes = (data_len as u32).to_le_bytes();
        let crc_bytes = crc.to_le_bytes();
        let seq_bytes = seq.to_le_bytes();

        // 1. Write Mmap
        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&len_bytes);
        self.cursor += 4;
        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&crc_bytes);
        self.cursor += 4;
        self.mmap[self.cursor..self.cursor + 8].copy_from_slice(&seq_bytes);
        self.cursor += 8;
        self.mmap[self.cursor..self.cursor + cmd_bytes.len()].copy_from_slice(&cmd_bytes);
        self.cursor += cmd_bytes.len();

        // 2. Update MD5
        self.md5_ctx.consume(len_bytes);
        self.md5_ctx.consume(crc_bytes);
        self.md5_ctx.consume(seq_bytes);
        self.md5_ctx.consume(&cmd_bytes);

        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.mmap.flush()?;
        Ok(())
    }

    pub fn finish_and_get_md5(self) -> String {
        format!("{:x}", self.md5_ctx.compute())
    }
}

// ==========================================
// 4. Rolling WAL Manager
// ==========================================

pub struct RollingWal {
    dir: PathBuf,
    current_segment: Option<WalSegment>,
    last_roll_time: SystemTime,
}

impl RollingWal {
    pub fn new(dir: &Path, start_seq: u64) -> Result<Self> {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }

        let segment = if let Some(last_file) = Self::find_latest_wal(dir)? {
            println!("   [WAL] Resuming from: {:?}", last_file);
            WalSegment::open_existing(&last_file)?
        } else {
            let path = dir.join(format!("ledger_{}.wal", start_seq));
            println!("   [WAL] Creating new: {:?}", path);
            WalSegment::create(&path)?
        };

        Ok(Self {
            dir: dir.to_path_buf(),
            current_segment: Some(segment),
            last_roll_time: SystemTime::now(),
        })
    }

    pub fn append(&mut self, seq: u64, cmd: &LedgerCommand) -> Result<()> {
        let time_trigger = SystemTime::now()
            .duration_since(self.last_roll_time)
            .unwrap_or(Duration::ZERO)
            >= WAL_ROLL_TIME;

        if time_trigger {
            println!("   [WAL] Trigger: Time limit. Rolling...");
            self.rotate(seq)?;
        }

        if self
            .current_segment
            .as_mut()
            .unwrap()
            .append(seq, cmd)
            .is_err()
        {
            self.rotate(seq)?;
            self.current_segment.as_mut().unwrap().append(seq, cmd)?;
        }
        Ok(())
    }

    fn rotate(&mut self, next_seq: u64) -> Result<()> {
        if let Some(mut old_segment) = self.current_segment.take() {
            old_segment.flush()?;

            // 1. Clone path BEFORE consuming segment
            let old_path = old_segment.current_path.clone();

            // 2. Consume segment to get MD5
            let md5_str = old_segment.finish_and_get_md5();

            // 3. Rename with MD5 suffix
            if let Some(stem) = old_path.file_stem().and_then(|s| s.to_str()) {
                if !stem.contains(&md5_str) {
                    // Prevent double renaming
                    let new_filename = format!("{}_{}.wal", stem, md5_str);
                    let new_path = self.dir.join(new_filename);
                    if old_path.exists() {
                        fs::rename(&old_path, &new_path)?;
                    }
                }
            }
        }

        // 4. Create new segment
        let new_path = self.dir.join(format!("ledger_{}.wal", next_seq));
        let new_segment = WalSegment::create(&new_path)?;

        self.current_segment = Some(new_segment);
        self.last_roll_time = SystemTime::now();

        // 5. Async Cleanup
        let dir = self.dir.clone();
        thread::spawn(move || {
            let _ = Self::cleanup_old_wals(&dir);
        });

        Ok(())
    }

    fn list_wals(dir: &Path) -> Result<Vec<(u64, PathBuf)>> {
        let mut wals = Vec::new();
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                // Matches "ledger_SEQ.wal" or "ledger_SEQ_MD5.wal"
                if name.starts_with("ledger_") && path.extension().is_some_and(|e| e == "wal") {
                    let parts: Vec<&str> = name.split('_').collect();
                    if parts.len() >= 2 {
                        if let Ok(seq) = parts[1].parse::<u64>() {
                            wals.push((seq, path));
                        }
                    }
                }
            }
        }
        wals.sort_by_key(|k| k.0);
        Ok(wals)
    }

    fn find_latest_wal(dir: &Path) -> Result<Option<PathBuf>> {
        let mut wals = Self::list_wals(dir)?;
        if wals.is_empty() {
            return Ok(None);
        }
        Ok(Some(wals.pop().unwrap().1))
    }

    fn cleanup_old_wals(dir: &Path) -> Result<()> {
        let wals = Self::list_wals(dir)?;
        if wals.len() > WAL_RETENTION {
            let to_delete = wals.len() - WAL_RETENTION;
            for (seq, path) in wals.iter().take(to_delete) {
                println!("   [Cleanup] Deleting old WAL seq: {}", seq);
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    pub fn replay_iter(
        dir: &Path,
        min_seq: u64,
    ) -> Result<impl Iterator<Item = Result<(u64, LedgerCommand)>>> {
        let wals = Self::list_wals(dir)?;
        let start_idx = wals
            .partition_point(|(seq, _)| *seq <= min_seq)
            .saturating_sub(1);

        let mut chained_iter = Vec::new();
        for (seq, path) in wals.iter().skip(start_idx) {
            match WalIterator::new(path) {
                Ok(iter) => {
                    println!(
                        "   [Recover] Queuing WAL segment: {:?} (Start: {})",
                        path.file_name().unwrap(),
                        seq
                    );
                    chained_iter.push(iter);
                }
                Err(e) => {
                    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                        if io_err.kind() == std::io::ErrorKind::NotFound {
                            println!("   [Recover] Skip missing file (cleaned up?): {:?}", path);
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }
        Ok(chained_iter.into_iter().flatten())
    }
}

// ==========================================
// 5. Streaming WAL Iterator
// ==========================================

pub struct WalIterator {
    reader: BufReader<File>,
    cursor: u64,
    path: PathBuf,
}

impl WalIterator {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::with_capacity(READ_BUFFER_SIZE, file);
        reader.seek(SeekFrom::Start(0))?;
        Ok(Self {
            reader,
            cursor: 0,
            path: path.to_path_buf(),
        })
    }
}

impl Iterator for WalIterator {
    type Item = Result<(u64, LedgerCommand)>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut len_buf = [0u8; 4];
        match self.reader.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
            Err(e) => return Some(Err(e.into())),
        }

        let payload_len = u32::from_le_bytes(len_buf) as usize;
        if payload_len > MAX_RECORD_SIZE {
            return Some(Err(anyhow::anyhow!("Record too large")));
        }
        if payload_len == 0 {
            return None;
        }

        let mut crc_buf = [0u8; 4];
        if let Err(e) = self.reader.read_exact(&mut crc_buf) {
            return Some(Err(e.into()));
        }
        let stored_crc = u32::from_le_bytes(crc_buf);

        let mut data_buf = vec![0u8; payload_len];
        if let Err(e) = self.reader.read_exact(&mut data_buf) {
            return Some(Err(e.into()));
        }

        let mut hasher = Hasher::new();
        hasher.update(&len_buf);
        hasher.update(&data_buf);
        if hasher.finalize() != stored_crc {
            return Some(Err(anyhow::anyhow!("CRC Mismatch in file {:?}", self.path)));
        }

        let (seq_bytes, cmd_bytes) = data_buf.split_at(8);
        let seq = u64::from_le_bytes(seq_bytes.try_into().unwrap());
        let cmd = match bincode::deserialize(cmd_bytes) {
            Ok(c) => c,
            Err(e) => return Some(Err(e.into())),
        };

        self.cursor += 8 + payload_len as u64;
        Some(Ok((seq, cmd)))
    }
}

// ==========================================
// 6. Global Ledger
// ==========================================

pub struct GlobalLedger {
    accounts: FxHashMap<UserId, UserAccount>,
    wal: RollingWal,
    pub last_seq: u64,
    snapshot_dir: PathBuf,
}

impl GlobalLedger {
    pub fn new(wal_dir: &Path, snapshot_dir: &Path) -> Result<Self> {
        fs::create_dir_all(wal_dir)?;
        fs::create_dir_all(snapshot_dir)?;

        let mut accounts = FxHashMap::default();
        let mut recovered_seq = 0;

        // 1. Load Snapshot (With MD5 check)
        if let Some((seq, path, expected_md5)) = Self::find_latest_snapshot(snapshot_dir)? {
            println!("   [Recover] Loading Snapshot: {:?} (Seq {})", path, seq);
            let file = File::open(&path)?;
            let mut md5_reader = Md5Reader::new(BufReader::new(file));
            let snap: Snapshot = bincode::deserialize_from(&mut md5_reader)?;

            if md5_reader.finish() != expected_md5 {
                bail!("Snapshot MD5 Mismatch");
            }

            accounts = snap.accounts;
            recovered_seq = snap.last_seq;
        }

        // 2. Replay WAL
        println!("   [Recover] Scanning WALs from Seq {}...", recovered_seq);
        let mut count = 0;

        for res in RollingWal::replay_iter(wal_dir, recovered_seq)? {
            let (seq, cmd) = res?;
            if seq <= recovered_seq {
                continue;
            }
            if seq != recovered_seq + 1 {
                bail!("Gap! Expected {}, Found {}", recovered_seq + 1, seq);
            }
            Self::apply_transaction(&mut accounts, &cmd)?;
            recovered_seq = seq;
            count += 1;
        }
        println!("   [Recover] Replay Done. {} txs.", count);

        let wal = RollingWal::new(wal_dir, recovered_seq + 1)?;

        Ok(Self {
            accounts,
            wal,
            last_seq: recovered_seq,
            snapshot_dir: snapshot_dir.to_path_buf(),
        })
    }

    /// Create Ledger from an existing state (e.g. from a unified snapshot)
    /// This skips the internal WAL replay and Snapshot loading of the Ledger itself.
    pub fn from_state(
        wal_dir: &Path,
        snapshot_dir: &Path,
        accounts: FxHashMap<UserId, UserAccount>,
        last_seq: u64,
    ) -> Result<Self> {
        fs::create_dir_all(wal_dir)?;
        fs::create_dir_all(snapshot_dir)?;

        // Initialize WAL for appending from the next sequence
        let wal = RollingWal::new(wal_dir, last_seq + 1)?;

        Ok(Self {
            accounts,
            wal,
            last_seq,
            snapshot_dir: snapshot_dir.to_path_buf(),
        })
    }

    fn find_latest_snapshot(dir: &Path) -> Result<Option<(u64, PathBuf, String)>> {
        let mut max_seq = 0;
        let mut found = None;
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                if name.starts_with("snapshot_") && path.extension().is_some_and(|e| e == "snap") {
                    let parts: Vec<&str> = name.split('_').collect();
                    if parts.len() == 3 {
                        if let Ok(seq) = parts[1].parse::<u64>() {
                            let md5 = parts[2].to_string();
                            if seq > max_seq {
                                max_seq = seq;
                                found = Some((seq, path, md5));
                            }
                        }
                    }
                }
            }
        }
        Ok(found)
    }

    // [COMPLETED CODE] Manual Snapshot with MD5 + N=3 Retention
    pub fn trigger_snapshot(&self) {
        let current_seq = self.last_seq;
        let dir = self.snapshot_dir.clone();
        let accounts_copy = self.accounts.clone();

        thread::spawn(move || {
            let tmp_filename = format!("snapshot_{}.tmp", current_seq);
            let tmp_path = dir.join(&tmp_filename);
            let snap = Snapshot {
                last_seq: current_seq,
                accounts: accounts_copy,
            };

            // 1. Write and Calculate MD5
            let md5_string = match File::create(&tmp_path) {
                Ok(file) => {
                    let buf_writer = BufWriter::new(file);
                    // Use the extracted Md5Writer utility
                    let mut md5_writer = Md5Writer::new(buf_writer);

                    if let Err(e) = bincode::serialize_into(&mut md5_writer, &snap) {
                        println!("   [Snapshot] Write Error: {:?}", e);
                        return;
                    }
                    // Flush to ensure all bytes are processed by MD5
                    if let Err(e) = md5_writer.flush() {
                        println!("   [Snapshot] Flush Error: {:?}", e);
                        return;
                    }
                    md5_writer.finish()
                }
                Err(e) => {
                    println!("   [Snapshot] Create File Error: {:?}", e);
                    return;
                }
            };

            // 2. Rename with MD5
            let final_filename = format!("snapshot_{}_{}.snap", current_seq, md5_string);
            let final_path = dir.join(final_filename);

            if let Err(e) = fs::rename(&tmp_path, &final_path) {
                println!("   [Snapshot] Rename Error: {:?}", e);
            } else {
                // println!("   [Snapshot] Saved: {:?}", final_path);
            }

            // 3. Cleanup Old Snapshots (Retention N=3)
            let mut snaps = Vec::new();
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                        if name.starts_with("snapshot_")
                            && p.extension().is_some_and(|e| e == "snap")
                        {
                            let parts: Vec<&str> = name.split('_').collect();
                            if parts.len() == 3 {
                                if let Ok(seq) = parts[1].parse::<u64>() {
                                    snaps.push((seq, p));
                                }
                            }
                        }
                    }
                }
            }
            snaps.sort_by(|a, b| b.0.cmp(&a.0)); // Descending by seq

            if snaps.len() > SNAPSHOT_RETENTION {
                for (seq, path) in snaps.iter().skip(SNAPSHOT_RETENTION) {
                    println!("   [Cleanup] Deleting old snapshot seq: {}", seq);
                    let _ = fs::remove_file(path);
                }
            }
        });
    }

    pub fn get_user_balances(&self, user_id: UserId) -> Option<Vec<(AssetId, Balance)>> {
        self.accounts.get(&user_id).map(|u| u.assets.clone())
    }

    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        let new_seq = self.last_seq + 1;
        self.wal.append(new_seq, cmd)?;
        Self::apply_transaction(&mut self.accounts, cmd)?;
        self.last_seq = new_seq;
        Ok(())
    }

    fn apply_transaction(
        accounts: &mut FxHashMap<UserId, UserAccount>,
        cmd: &LedgerCommand,
    ) -> Result<()> {
        match cmd {
            LedgerCommand::Deposit {
                user_id,
                asset,
                amount,
            } => {
                let user = accounts
                    .entry(*user_id)
                    .or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset);
                bal.available += amount;
            }
            LedgerCommand::Withdraw {
                user_id,
                asset,
                amount,
            } => {
                let user = accounts
                    .entry(*user_id)
                    .or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount {
                    anyhow::bail!(
                        "Insufficient funds for withdraw: User {} Asset {}",
                        user_id,
                        asset
                    );
                }
                bal.available -= amount;
            }
            LedgerCommand::Lock {
                user_id,
                asset,
                amount,
            } => {
                let user = accounts
                    .entry(*user_id)
                    .or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount {
                    anyhow::bail!(
                        "Insufficient funds for lock: User {} Asset {}",
                        user_id,
                        asset
                    );
                }
                bal.available -= amount;
                bal.frozen += amount;
            }
            LedgerCommand::Unlock {
                user_id,
                asset,
                amount,
            } => {
                let user = accounts
                    .entry(*user_id)
                    .or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset);
                if bal.frozen < *amount {
                    anyhow::bail!(
                        "Insufficient frozen funds for unlock: User {} Asset {}",
                        user_id,
                        asset
                    );
                }
                bal.frozen -= amount;
                bal.available += amount;
            }
            LedgerCommand::TradeSettle {
                user_id,
                spend_asset,
                spend_amount,
                gain_asset,
                gain_amount,
            } => {
                let user = accounts
                    .entry(*user_id)
                    .or_insert_with(|| UserAccount::new(*user_id));

                // Use indices to avoid multiple mutable borrows
                let spend_idx = user.assets.iter().position(|(a, _)| *a == *spend_asset);

                if let Some(idx) = spend_idx {
                    if user.assets[idx].1.frozen < *spend_amount {
                        anyhow::bail!(
                            "Insufficient frozen funds for settle: User {} Asset {}",
                            user_id,
                            spend_asset
                        );
                    }
                    user.assets[idx].1.frozen -= spend_amount;
                } else {
                    anyhow::bail!(
                        "Asset not found for spend: User {} Asset {}",
                        user_id,
                        spend_asset
                    );
                }

                let gain_idx = user.assets.iter().position(|(a, _)| *a == *gain_asset);
                if let Some(idx) = gain_idx {
                    user.assets[idx].1.available += gain_amount;
                } else {
                    user.assets.push((
                        *gain_asset,
                        Balance {
                            available: *gain_amount,
                            frozen: 0,
                        },
                    ));
                }
            }
            LedgerCommand::MatchExec(data) => {
                Self::apply_match_exec(accounts, data)?;
            }
            LedgerCommand::MatchExecBatch(batch) => {
                for data in batch {
                    Self::apply_match_exec(accounts, data)?;
                }
            }
            LedgerCommand::Batch(cmds) => {
                for cmd in cmds {
                    Self::apply_transaction(accounts, cmd)?;
                }
            }
        }
        Ok(())
    }

    fn apply_match_exec(
        accounts: &mut FxHashMap<UserId, UserAccount>,
        data: &MatchExecData,
    ) -> Result<()> {
        let buyer_spend = data.price * data.quantity;
        let buyer_gain = data.quantity;
        let seller_spend = data.quantity;
        let seller_gain = data.price * data.quantity;

        // 1. Buyer Settle
        let buyer = accounts
            .entry(data.buyer_user_id)
            .or_insert_with(|| UserAccount::new(data.buyer_user_id));

        // Debit Quote (Frozen)
        let quote_bal = buyer.get_balance_mut(data.quote_asset);
        if quote_bal.frozen < buyer_spend {
            anyhow::bail!("Insufficient frozen quote for buyer {}", data.buyer_user_id);
        }
        quote_bal.frozen -= buyer_spend;

        // Credit Base (Available)
        let base_bal = buyer.get_balance_mut(data.base_asset);
        base_bal.available += buyer_gain;

        // Refund Quote (Frozen -> Available)
        if data.buyer_refund > 0 {
            let quote_bal = buyer.get_balance_mut(data.quote_asset);
            if quote_bal.frozen < data.buyer_refund {
                anyhow::bail!(
                    "Insufficient frozen quote for refund buyer {}",
                    data.buyer_user_id
                );
            }
            quote_bal.frozen -= data.buyer_refund;
            quote_bal.available += data.buyer_refund;
        }

        // 2. Seller Settle
        let seller = accounts
            .entry(data.seller_user_id)
            .or_insert_with(|| UserAccount::new(data.seller_user_id));

        // Debit Base (Frozen)
        let base_bal = seller.get_balance_mut(data.base_asset);
        if base_bal.frozen < seller_spend {
            anyhow::bail!(
                "Insufficient frozen base for seller {}",
                data.seller_user_id
            );
        }
        base_bal.frozen -= seller_spend;

        // Credit Quote (Available)
        let quote_bal = seller.get_balance_mut(data.quote_asset);
        quote_bal.available += seller_gain;

        // Refund Base (Frozen -> Available) - unlikely but supported
        if data.seller_refund > 0 {
            let base_bal = seller.get_balance_mut(data.base_asset);
            if base_bal.frozen < data.seller_refund {
                anyhow::bail!(
                    "Insufficient frozen base for refund seller {}",
                    data.seller_user_id
                );
            }
            base_bal.frozen -= data.seller_refund;
            base_bal.available += data.seller_refund;
        }
        Ok(())
    }
    pub fn get_accounts(&self) -> &FxHashMap<UserId, UserAccount> {
        &self.accounts
    }
}

// ==========================================
// 7. Main
// ==========================================
