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
use crate::user_account::{AssetId, Balance, UserAccount, UserId};

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

// Snapshot on disk
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub last_seq: u64,
    pub accounts: FxHashMap<UserId, UserAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// LedgerCommand represents operations on the ledger.
///
/// **Note**: Many of these variants are being deprecated in favor of `EngineOutput`.
/// The new `EngineOutput` structure provides atomic, verifiable, chain-linked bundles.
/// See `src/engine_output.rs` and `docs/ENGINE_OUTPUT_REFACTOR.md` for details.
pub enum LedgerCommand {
    /// Deposit funds to user's available balance
    Deposit { user_id: UserId, asset_id: AssetId, amount: u64, balance_after: u64, version: u64 },
    /// Withdraw funds from user's available balance
    Withdraw { user_id: UserId, asset_id: AssetId, amount: u64, balance_after: u64, version: u64 },
    /// Lock funds (move from available to frozen)
    Lock { user_id: UserId, asset_id: AssetId, amount: u64, balance_after: u64, version: u64 },
    /// Unlock funds (move from frozen to available)
    Unlock { user_id: UserId, asset_id: AssetId, amount: u64, balance_after: u64, version: u64 },
    /// **DEPRECATED**: Use `EngineOutput.trades` instead.
    /// This variant was never used in production.
    #[deprecated(note = "Use EngineOutput.trades instead")]
    TradeSettle {
        user_id: UserId,
        spend_asset_id: AssetId,
        spend_amount: u64,
        gain_asset_id: AssetId,
        gain_amount: u64,
    },
    /// Single match execution - kept for backward compatibility
    /// Prefer `MatchExecBatch` or `EngineOutput.trades`
    MatchExec(MatchExecData),
    /// Batch of match executions - current production format
    /// Will be replaced by `EngineOutput` in future
    MatchExecBatch(Vec<MatchExecData>),
    /// Order status update
    OrderUpdate(OrderUpdate),
    /// Batch of commands for atomic processing
    Batch(Vec<LedgerCommand>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchExecData {
    pub trade_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub buyer_user_id: UserId,
    pub seller_user_id: UserId,
    pub price: u64,
    pub quantity: u64,
    pub base_asset_id: AssetId,
    pub quote_asset_id: AssetId,
    pub buyer_refund: u64,
    pub seller_refund: u64,
    pub match_seq: u64,
    pub output_sequence: u64,
    pub settled_at: u64,
    // Balance versions from matching engine (for idempotent settlement)
    pub buyer_quote_version: u64, // buyer's quote_asset_id balance version
    pub buyer_base_version: u64,  // buyer's base_asset_id balance version
    pub seller_base_version: u64, // seller's base_asset_id balance version
    pub seller_quote_version: u64, // seller's quote_asset_id balance version

    // Balance snapshots for clearing log
    pub buyer_quote_balance_after: u64,
    pub buyer_base_balance_after: u64,
    pub seller_base_balance_after: u64,
    pub seller_quote_balance_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub order_id: u64,
    pub client_order_id: Option<String>,
    pub user_id: UserId,
    pub symbol_id: u32,
    pub side: u8,       // 1=Buy, 2=Sell
    pub order_type: u8, // 1=Limit, 2=Market
    pub status: OrderStatus,
    pub price: u64,
    pub qty: u64,
    pub filled_qty: u64,
    pub avg_fill_price: Option<u64>,
    pub rejection_reason: Option<String>,
    pub timestamp: u64,        // ms
    pub match_id: Option<u64>, // For Fills
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEvent {
    pub user_id: UserId,
    pub sequence_id: u64,
    pub event_type: String,
    pub amount: u64,
    pub currency: AssetId,
    pub related_id: u64,
    pub created_at: i64,
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
        let file =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
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

        Ok(Self { mmap, cursor, len, md5_ctx, current_path: path.to_path_buf() })
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
        let time_trigger =
            SystemTime::now().duration_since(self.last_roll_time).unwrap_or(Duration::ZERO)
                >= WAL_ROLL_TIME;

        if time_trigger {
            println!("   [WAL] Trigger: Time limit. Rolling...");
            self.rotate(seq)?;
        }

        if self.current_segment.as_mut().unwrap().append(seq, cmd).is_err() {
            self.rotate(seq)?;
            self.current_segment.as_mut().unwrap().append(seq, cmd)?;
        }
        Ok(())
    }

    pub fn append_no_flush(&mut self, seq: u64, cmd: &LedgerCommand) -> Result<()> {
        self.append(seq, cmd)
    }

    pub fn flush(&mut self) -> Result<()> {
        if let Some(segment) = &mut self.current_segment {
            segment.flush()?;
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
        let start_idx = wals.partition_point(|(seq, _)| *seq <= min_seq).saturating_sub(1);

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
        Ok(Self { reader, cursor: 0, path: path.to_path_buf() })
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
// Ledger Trait and Shadow Implementation
// ==========================================

pub trait Ledger {
    fn get_balance(&self, user_id: UserId, asset_id: AssetId) -> u64;
    fn get_frozen(&self, user_id: UserId, asset_id: AssetId) -> u64;
    fn get_balance_version(&self, user_id: UserId, asset_id: AssetId) -> u64;
    fn apply(&mut self, cmd: &LedgerCommand) -> Result<()>;
}

/// A temporary, in-memory ledger that buffers changes before they are committed.
///
/// Implements a Copy-On-Write (CoW) mechanism:
/// - Reads check `delta_accounts` first, then fall back to `real_ledger`.
/// - Writes clone the account from `real_ledger` to `delta_accounts` before modifying.
///
/// This allows validating a batch of commands atomically without side effects.
pub struct ShadowLedger<'a> {
    real_ledger: &'a GlobalLedger,
    delta_accounts: FxHashMap<UserId, UserAccount>,
    pub pending_commands: Vec<LedgerCommand>,
}

impl<'a> ShadowLedger<'a> {
    pub fn new(real_ledger: &'a GlobalLedger) -> Self {
        Self { real_ledger, delta_accounts: FxHashMap::default(), pending_commands: Vec::new() }
    }

    /// Consumes the ShadowLedger and returns the modified accounts.
    /// This releases the borrow on the real_ledger.
    pub fn into_delta(self) -> (FxHashMap<UserId, UserAccount>, Vec<LedgerCommand>) {
        (self.delta_accounts, self.pending_commands)
    }
}

impl<'a> Ledger for ShadowLedger<'a> {
    fn get_balance(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        if let Some(account) = self.delta_accounts.get(&user_id) {
            return account
                .assets()
                .iter()
                .find(|(a, _)| *a == asset_id)
                .map(|(_, b)| b.avail())
                .unwrap_or(0);
        }
        self.real_ledger.get_balance(user_id, asset_id)
    }

    fn get_frozen(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        if let Some(account) = self.delta_accounts.get(&user_id) {
            return account
                .assets()
                .iter()
                .find(|(a, _)| *a == asset_id)
                .map(|(_, b)| b.frozen())
                .unwrap_or(0);
        }
        self.real_ledger.get_frozen(user_id, asset_id)
    }

    fn get_balance_version(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        if let Some(account) = self.delta_accounts.get(&user_id) {
            return account
                .assets()
                .iter()
                .find(|(a, _)| *a == asset_id)
                .map(|(_, b)| b.version())
                .unwrap_or(0);
        }
        self.real_ledger.get_balance_version(user_id, asset_id)
    }

    fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        // Apply to delta state
        match cmd {
            LedgerCommand::Lock { user_id, .. }
            | LedgerCommand::Unlock { user_id, .. }
            | LedgerCommand::Deposit { user_id, .. }
            | LedgerCommand::Withdraw { user_id, .. }
            | LedgerCommand::TradeSettle { user_id, .. } => {
                let account = self
                    .delta_accounts
                    .entry(*user_id)
                    .or_insert_with(|| self.real_ledger.get_account_copy(*user_id));

                GlobalLedger::apply_transaction(&mut self.delta_accounts, cmd)?;
            }
            LedgerCommand::MatchExec(data) => {
                self.delta_accounts
                    .entry(data.buyer_user_id)
                    .or_insert_with(|| self.real_ledger.get_account_copy(data.buyer_user_id));
                self.delta_accounts
                    .entry(data.seller_user_id)
                    .or_insert_with(|| self.real_ledger.get_account_copy(data.seller_user_id));
                GlobalLedger::apply_transaction(&mut self.delta_accounts, cmd)?;
            }
            LedgerCommand::MatchExecBatch(batch) => {
                for data in batch {
                    self.delta_accounts
                        .entry(data.buyer_user_id)
                        .or_insert_with(|| self.real_ledger.get_account_copy(data.buyer_user_id));
                    self.delta_accounts
                        .entry(data.seller_user_id)
                        .or_insert_with(|| self.real_ledger.get_account_copy(data.seller_user_id));
                }
                GlobalLedger::apply_transaction(&mut self.delta_accounts, cmd)?;
            }
            LedgerCommand::Batch(cmds) => {
                for c in cmds {
                    self.apply(c)?;
                }
                // Return early for Batch since recursive calls handle pushing
                return Ok(());
            }
            LedgerCommand::OrderUpdate(_) => {} // No-op for ledger state
        }

        // Record command ONLY if application succeeded
        self.pending_commands.push(cmd.clone());
        Ok(())
    }
}

// ==========================================
// 6. Global Ledger
// ==========================================

pub trait LedgerListener: Send + Sync {
    fn on_command(&mut self, cmd: &LedgerCommand) -> Result<()>;

    fn on_batch(&mut self, cmds: &[LedgerCommand]) -> Result<()> {
        self.on_command(&LedgerCommand::Batch(cmds.to_vec()))
    }
}

pub struct GlobalLedger {
    accounts: FxHashMap<UserId, UserAccount>,
    wal: Option<RollingWal>,
    pub last_seq: u64,
    snapshot_dir: PathBuf,
    listener: Option<Box<dyn LedgerListener>>,
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
            wal: Some(wal),
            last_seq: recovered_seq,
            snapshot_dir: snapshot_dir.to_path_buf(),
            listener: None,
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
            wal: Some(wal),
            last_seq,
            snapshot_dir: snapshot_dir.to_path_buf(),
            listener: None,
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
            let snap = Snapshot { last_seq: current_seq, accounts: accounts_copy };

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
        self.accounts.get(&user_id).map(|u| u.assets().to_vec())
    }

    pub fn get_account_copy(&self, user_id: UserId) -> UserAccount {
        self.accounts.get(&user_id).cloned().unwrap_or_else(|| UserAccount::new(user_id))
    }

    pub fn set_listener(&mut self, listener: Box<dyn LedgerListener>) {
        self.listener = Some(listener);
    }

    // Inherent apply (already exists below)
    // pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> ...
}

impl Ledger for GlobalLedger {
    fn get_balance(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        self.accounts
            .get(&user_id)
            .and_then(|u| u.assets().iter().find(|(a, _)| *a == asset_id))
            .map(|(_, b)| b.avail())
            .unwrap_or(0)
    }

    fn get_frozen(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        self.accounts
            .get(&user_id)
            .and_then(|u| u.assets().iter().find(|(a, _)| *a == asset_id))
            .map(|(_, b)| b.frozen())
            .unwrap_or(0)
    }

    fn get_balance_version(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        self.accounts
            .get(&user_id)
            .and_then(|u| u.assets().iter().find(|(a, _)| *a == asset_id))
            .map(|(_, b)| b.version())
            .unwrap_or(0)
    }

    fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        self.apply(cmd)
    }
}

impl GlobalLedger {
    pub fn take_wal(&mut self) -> Option<RollingWal> {
        self.wal.take()
    }

    pub fn apply_delta_to_memory(&mut self, delta: FxHashMap<UserId, UserAccount>) {
        for (user_id, account) in delta {
            self.accounts.insert(user_id, account);
        }
    }

    pub fn append_to_wal(&mut self, cmds: &[LedgerCommand]) -> Result<()> {
        if let Some(wal) = &mut self.wal {
            for cmd in cmds {
                let new_seq = self.last_seq + 1;
                wal.append_no_flush(new_seq, cmd)?;
                self.last_seq = new_seq;
            }
            // wal.flush()?; // OPTIMIZATION: Skip flush, rely on Order WAL for durability
        }
        Ok(())
    }

    pub fn commit_delta(
        &mut self,
        cmds: &[LedgerCommand],
        delta: FxHashMap<UserId, UserAccount>,
    ) -> Result<()> {
        self.append_to_wal(cmds)?;
        self.apply_delta_to_memory(delta);

        // Notify Listeners (if any)
        if let Some(listener) = &mut self.listener {
            listener.on_batch(cmds)?;
        }
        Ok(())
    }

    pub fn commit_batch(&mut self, cmds: &[LedgerCommand]) -> Result<()> {
        // Phase 1: Validate all commands using shadow ledger (no side effects)
        // This ensures atomicity: if any command fails here, we abort before writing to WAL.
        let start_validate = std::time::Instant::now();
        let mut shadow = ShadowLedger::new(self);
        for cmd in cmds {
            shadow.apply(cmd)?;
        }
        // Optimization: Capture the calculated state changes
        let (delta, _) = shadow.into_delta();
        let validate_duration = start_validate.elapsed();

        self.commit_delta(cmds, delta)?;

        println!("[PERF] Commit Batch: Validate: {:?}", validate_duration);
        Ok(())
    }

    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        let new_seq = self.last_seq + 1;
        if let Some(wal) = &mut self.wal {
            wal.append(new_seq, cmd)?;
        }
        Self::apply_transaction(&mut self.accounts, cmd)?;

        if let Some(listener) = &mut self.listener {
            listener.on_command(cmd)?;
        }

        self.last_seq = new_seq;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        if let Some(wal) = &mut self.wal {
            wal.flush()
        } else {
            Ok(())
        }
    }

    pub fn apply_transaction(
        accounts: &mut FxHashMap<UserId, UserAccount>,
        cmd: &LedgerCommand,
    ) -> Result<()> {
        match cmd {
            LedgerCommand::Deposit { user_id, asset_id, amount, .. } => {
                let user = accounts.entry(*user_id).or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset_id);
                bal.deposit(*amount).map_err(|e| {
                    anyhow::anyhow!("Deposit failed for User {} Asset {}: {}", user_id, asset_id, e)
                })?;
            }
            LedgerCommand::Withdraw { user_id, asset_id, amount, .. } => {
                let user = accounts.entry(*user_id).or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset_id);
                bal.withdraw(*amount).map_err(|_| {
                    anyhow::anyhow!(
                        "Insufficient funds for withdraw: User {} Asset {}",
                        user_id,
                        asset_id
                    )
                })?;
            }
            LedgerCommand::Lock { user_id, asset_id, amount, .. } => {
                let user = accounts.entry(*user_id).or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset_id);
                bal.lock(*amount).map_err(|_| {
                    anyhow::anyhow!(
                        "Insufficient funds for lock: User {} Asset {}",
                        user_id,
                        asset_id
                    )
                })?;
            }
            LedgerCommand::Unlock { user_id, asset_id, amount, .. } => {
                let user = accounts.entry(*user_id).or_insert_with(|| UserAccount::new(*user_id));
                let bal = user.get_balance_mut(*asset_id);
                bal.unlock(*amount).map_err(|_| {
                    anyhow::anyhow!(
                        "Insufficient frozen funds for unlock: User {} Asset {}",
                        user_id,
                        asset_id
                    )
                })?;
            }
            LedgerCommand::TradeSettle {
                user_id,
                spend_asset_id,
                spend_amount,
                gain_asset_id,
                gain_amount,
            } => {
                let user = accounts.entry(*user_id).or_insert_with(|| UserAccount::new(*user_id));

                // Use indices to avoid multiple mutable borrows
                let spend_idx = user.assets().iter().position(|(a, _)| *a == *spend_asset_id);

                if let Some(idx) = spend_idx {
                    user.assets_mut()[idx].1.spend_frozen(*spend_amount).map_err(|_| {
                        anyhow::anyhow!(
                            "Insufficient frozen funds for settle: User {} Asset {}",
                            user_id,
                            spend_asset_id
                        )
                    })?;
                } else {
                    anyhow::bail!(
                        "Asset not found for spend: User {} Asset {}",
                        user_id,
                        spend_asset_id
                    );
                }

                let gain_idx = user.assets().iter().position(|(a, _)| *a == *gain_asset_id);
                if let Some(idx) = gain_idx {
                    let _ = user.assets_mut()[idx].1.deposit(*gain_amount);
                } else {
                    user.assets_mut().push((
                        *gain_asset_id,
                        Balance::default(),
                    ));
                    // Deposit to the newly created balance
                    let last_idx = user.assets_mut().len() - 1;
                    let _ = user.assets_mut()[last_idx].1.deposit(*gain_amount);
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
            LedgerCommand::OrderUpdate(_) => {} // No-op for ledger state
        }
        Ok(())
    }

    fn apply_match_exec(
        accounts: &mut FxHashMap<UserId, UserAccount>,
        data: &MatchExecData,
    ) -> Result<()> {
        let quote_amount = data
            .price
            .checked_mul(data.quantity)
            .ok_or_else(|| anyhow::anyhow!("Quote amount overflow"))?;
        let base_amount = data.quantity;

        // 1. Pre-flight Checks (Read-Only)
        // Check Buyer
        let buyer = accounts
            .get(&data.buyer_user_id)
            .ok_or_else(|| anyhow::anyhow!("Buyer account {} not found", data.buyer_user_id))?;

        buyer
            .check_buyer_balance(data.quote_asset_id, quote_amount, data.buyer_refund)
            .map_err(|e| anyhow::anyhow!("Buyer check failed: {}", e))?;

        // Check Seller
        let seller = accounts
            .get(&data.seller_user_id)
            .ok_or_else(|| anyhow::anyhow!("Seller account {} not found", data.seller_user_id))?;

        seller
            .check_seller_balance(data.base_asset_id, base_amount, data.seller_refund)
            .map_err(|e| anyhow::anyhow!("Seller check failed: {}", e))?;

        // 2. Execute Settlement (Guaranteed to succeed logic-wise)
        // Buyer Settle
        let buyer = accounts.get_mut(&data.buyer_user_id).unwrap();
        buyer
            .settle_as_buyer(
                data.quote_asset_id,
                data.base_asset_id,
                quote_amount,
                base_amount,
                data.buyer_refund,
            )
            .expect("Critical: Buyer settle failed after check passed");

        // Seller Settle
        let seller = accounts.get_mut(&data.seller_user_id).unwrap();
        seller
            .settle_as_seller(
                data.base_asset_id,
                data.quote_asset_id,
                base_amount,
                quote_amount,
                data.seller_refund,
            )
            .expect("Critical: Seller settle failed after check passed");
        Ok(())
    }
    pub fn get_accounts(&self) -> &FxHashMap<UserId, UserAccount> {
        &self.accounts
    }
}

// ==========================================
// 7. Main
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ==========================================
    // Helper Functions
    // ==========================================

    fn create_test_ledger() -> (GlobalLedger, TempDir, TempDir) {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let ledger = GlobalLedger::new(wal_dir.path(), snap_dir.path()).unwrap();
        (ledger, wal_dir, snap_dir)
    }

    fn setup_user_with_balance(
        ledger: &mut GlobalLedger,
        user_id: UserId,
        asset_id: AssetId,
        amount: u64,
    ) {
        ledger
            .apply(&LedgerCommand::Deposit {
                user_id,
                asset_id,
                amount,
                balance_after: 0,
                version: 0,
            })
            .unwrap();
    }

    // ==========================================
    // 1. Data Corruption Prevention Tests
    // ==========================================

    // ==========================================
    // 3. Overflow/Underflow Protection Tests
    // ==========================================

    #[test]
    fn test_deposit_overflow_protection() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        setup_user_with_balance(&mut ledger, 1, 100, u64::MAX - 100);

        // Try to deposit more than would fit
        let result = ledger.apply(&LedgerCommand::Deposit {
            user_id: 1,
            asset_id: 100,
            amount: 200,
            balance_after: 0,
            version: 0,
        });

        assert!(result.is_err(), "Should prevent overflow");
    }

    #[test]
    fn test_withdraw_underflow_protection() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        setup_user_with_balance(&mut ledger, 1, 100, 1000);

        let result = ledger.apply(&LedgerCommand::Withdraw {
            user_id: 1,
            asset_id: 100,
            amount: 1001,
            balance_after: 0,
            version: 0,
        });

        assert!(result.is_err(), "Should prevent underflow");
        assert_eq!(ledger.get_balance(1, 100), 1000, "Balance unchanged");
    }

    #[test]
    fn test_lock_insufficient_funds() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        setup_user_with_balance(&mut ledger, 1, 100, 500);

        let result = ledger.apply(&LedgerCommand::Lock {
            user_id: 1,
            asset_id: 100,
            amount: 600,
            balance_after: 0,
            version: 0,
        });

        assert!(result.is_err(), "Should fail with insufficient funds");
    }

    #[test]
    fn test_unlock_insufficient_frozen() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        setup_user_with_balance(&mut ledger, 1, 100, 1000);
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 1,
                asset_id: 100,
                amount: 300,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        let result = ledger.apply(&LedgerCommand::Unlock {
            user_id: 1,
            asset_id: 100,
            amount: 400,
            balance_after: 0,
            version: 0,
        });

        assert!(result.is_err(), "Should fail with insufficient frozen");
    }

    #[test]
    fn test_match_exec_overflow_detection() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // Setup with max values
        setup_user_with_balance(&mut ledger, 1, 200, u64::MAX);
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 1,
                asset_id: 200,
                amount: u64::MAX,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        setup_user_with_balance(&mut ledger, 2, 100, u64::MAX);
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 2,
                asset_id: 100,
                amount: u64::MAX,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Try to execute match that would overflow
        let match_data = MatchExecData {
            trade_id: 1,
            buy_order_id: 100,
            sell_order_id: 200,
            buyer_user_id: 1,
            seller_user_id: 2,
            price: u64::MAX,
            quantity: u64::MAX,
            base_asset_id: 100,
            quote_asset_id: 200,
            buyer_refund: 0,
            seller_refund: 0,
            match_seq: 1,
            output_sequence: 1,
            settled_at: 0,
            buyer_quote_version: 0,
            buyer_base_version: 0,
            seller_base_version: 0,
            seller_quote_version: 0,
            buyer_quote_balance_after: 0,
            buyer_base_balance_after: 0,
            seller_base_balance_after: 0,
            seller_quote_balance_after: 0,
        };

        let result = ledger.apply(&LedgerCommand::MatchExec(match_data));
        assert!(result.is_err(), "Should detect overflow in match execution");
    }

    // ==========================================
    // 4. WAL Recovery and Persistence Tests
    // ==========================================

    #[test]
    fn test_wal_recovery_preserves_order() {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();

        // Create sequence of operations
        {
            let mut ledger = GlobalLedger::new(wal_dir.path(), snap_dir.path()).unwrap();

            for i in 1..=10 {
                setup_user_with_balance(&mut ledger, i, 100, i * 1000);
            }

            ledger.flush().unwrap();
        }

        // Recover and verify
        let ledger = GlobalLedger::new(wal_dir.path(), snap_dir.path()).unwrap();

        for i in 1..=10 {
            assert_eq!(ledger.get_balance(i, 100), i * 1000, "Balance for user {} incorrect", i);
        }
    }

    #[test]
    fn test_snapshot_and_wal_recovery() {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();

        let final_seq;

        // Create state, snapshot, then add more
        {
            let mut ledger = GlobalLedger::new(wal_dir.path(), snap_dir.path()).unwrap();

            // Initial state
            setup_user_with_balance(&mut ledger, 1, 100, 5000);
            setup_user_with_balance(&mut ledger, 2, 200, 3000);

            // Snapshot
            ledger.trigger_snapshot();
            std::thread::sleep(std::time::Duration::from_millis(100));

            // More operations after snapshot
            setup_user_with_balance(&mut ledger, 3, 300, 7000);
            setup_user_with_balance(&mut ledger, 1, 100, 2000); // Additional deposit

            final_seq = ledger.last_seq;
            ledger.flush().unwrap();
        }

        // Recover - should load snapshot + replay WAL
        let ledger = GlobalLedger::new(wal_dir.path(), snap_dir.path()).unwrap();

        assert_eq!(ledger.last_seq, final_seq);
        assert_eq!(ledger.get_balance(1, 100), 7000); // 5000 + 2000
        assert_eq!(ledger.get_balance(2, 200), 3000);
        assert_eq!(ledger.get_balance(3, 300), 7000);
    }

    #[test]
    fn test_wal_rotation() {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();

        let mut ledger = GlobalLedger::new(wal_dir.path(), snap_dir.path()).unwrap();

        // Write enough data to trigger rotation
        for i in 0..1000 {
            setup_user_with_balance(&mut ledger, i, 100, 1000);
        }

        ledger.flush().unwrap();

        // Check that WAL files exist
        let wal_files: Vec<_> = std::fs::read_dir(wal_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|s| s == "wal").unwrap_or(false))
            .collect();

        // Should have at least one WAL file
        assert!(!wal_files.is_empty(), "No WAL files created");
    }

    // ==========================================
    // 5. Performance Tests
    // ==========================================

    #[test]
    fn test_batch_performance() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // Setup users
        for i in 1..=100 {
            setup_user_with_balance(&mut ledger, i, 100, 100000);
        }

        // Create large batch
        let mut batch = Vec::new();
        for i in 1..=100 {
            batch.push(LedgerCommand::Withdraw {
                user_id: i,
                asset_id: 100,
                amount: 100,
                balance_after: 0,
                version: 0,
            });
        }

        let start = std::time::Instant::now();
        ledger.commit_batch(&batch).unwrap();
        let duration = start.elapsed();

        println!("Batch of 100 operations took: {:?}", duration);

        // Verify all operations applied
        for i in 1..=100 {
            assert_eq!(ledger.get_balance(i, 100), 99900);
        }

        // Should complete in reasonable time (< 100ms for 100 ops)
        assert!(duration.as_millis() < 100, "Batch processing too slow: {:?}", duration);
    }

    #[test]
    fn test_shadow_ledger_performance() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // Setup base state
        for i in 1..=50 {
            setup_user_with_balance(&mut ledger, i, 100, 100000);
        }

        let start = std::time::Instant::now();

        // Use shadow ledger for batch validation
        let mut shadow = ShadowLedger::new(&ledger);

        for i in 1..=50 {
            shadow
                .apply(&LedgerCommand::Withdraw {
                    user_id: i,
                    asset_id: 100,
                    amount: 1000,
                    balance_after: 0,
                    version: 0,
                })
                .unwrap();
        }

        let duration = start.elapsed();

        println!("Shadow ledger 50 operations took: {:?}", duration);

        // Verify shadow state
        for i in 1..=50 {
            assert_eq!(shadow.get_balance(i, 100), 99000);
        }

        // Real ledger unchanged
        for i in 1..=50 {
            assert_eq!(ledger.get_balance(i, 100), 100000);
        }

        // Should be fast (< 10ms)
        assert!(duration.as_millis() < 10, "Shadow ledger too slow: {:?}", duration);
    }

    #[test]
    fn test_concurrent_shadow_ledgers() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        setup_user_with_balance(&mut ledger, 1, 100, 100000);

        let ledger = Arc::new(ledger);

        // Create multiple shadow ledgers concurrently
        let mut handles = vec![];

        for thread_id in 0..4 {
            let ledger_clone = Arc::clone(&ledger);

            let handle = std::thread::spawn(move || {
                let mut shadow = ShadowLedger::new(&ledger_clone);

                for _ in 0..100 {
                    shadow
                        .apply(&LedgerCommand::Withdraw {
                            user_id: 1,
                            asset_id: 100,
                            amount: 10,
                            balance_after: 0,
                            version: 0,
                        })
                        .unwrap();
                }

                // Each shadow should see its own state
                assert_eq!(shadow.get_balance(1, 100), 99000);
                thread_id
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Real ledger should be unchanged
        assert_eq!(ledger.get_balance(1, 100), 100000);
    }

    #[test]
    fn test_wal_write_throughput() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        let num_ops = 10000;
        let start = std::time::Instant::now();

        for i in 0..num_ops {
            ledger
                .apply(&LedgerCommand::Deposit {
                    user_id: 1,
                    asset_id: 100,
                    amount: 1,
                    balance_after: 0,
                    version: 0,
                })
                .unwrap();
        }

        ledger.flush().unwrap();
        let duration = start.elapsed();

        let ops_per_sec = num_ops as f64 / duration.as_secs_f64();

        println!("WAL throughput: {:.0} ops/sec ({} ops in {:?})", ops_per_sec, num_ops, duration);

        // Verify final balance
        assert_eq!(ledger.get_balance(1, 100), num_ops);

        // Should achieve reasonable throughput (> 10k ops/sec)
        assert!(ops_per_sec > 10000.0, "WAL throughput too low: {:.0} ops/sec", ops_per_sec);
    }

    // ==========================================
    // 6. Edge Cases and Complex Scenarios
    // ==========================================

    #[test]
    fn test_multiple_assets_per_user() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // User with multiple assets
        for asset_id in 100..110 {
            setup_user_with_balance(&mut ledger, 1, asset_id, asset_id as u64 * 1000);
        }

        // Verify all balances
        for asset_id in 100..110 {
            assert_eq!(ledger.get_balance(1, asset_id), asset_id as u64 * 1000);
        }
    }

    #[test]
    fn test_complex_trade_scenario() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // Setup multiple users with different assets
        setup_user_with_balance(&mut ledger, 1, 200, 100000); // Buyer with USDT
        setup_user_with_balance(&mut ledger, 2, 100, 50); // Seller with BTC

        // Buyer locks quote
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 1,
                asset_id: 200,
                amount: 60000,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Seller locks base
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 2,
                asset_id: 100,
                amount: 30,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Execute partial fill with refund
        let match_data = MatchExecData {
            trade_id: 1,
            buy_order_id: 100,
            sell_order_id: 200,
            buyer_user_id: 1,
            seller_user_id: 2,
            price: 2000,
            quantity: 20,
            base_asset_id: 100,
            quote_asset_id: 200,
            buyer_refund: 20000, // Refund unused quote
            seller_refund: 10,   // Refund unused base
            match_seq: 1,
            output_sequence: 1,
            settled_at: 0,
            buyer_quote_version: 0,
            buyer_base_version: 0,
            seller_base_version: 0,
            seller_quote_version: 0,
            buyer_quote_balance_after: 0,
            buyer_base_balance_after: 0,
            seller_base_balance_after: 0,
            seller_quote_balance_after: 0,
        };

        ledger.apply(&LedgerCommand::MatchExec(match_data)).unwrap();

        // Verify buyer state
        let buyer_balances = ledger.get_user_balances(1).unwrap();
        let buyer_quote = buyer_balances.iter().find(|(a, _)| *a == 200).unwrap().1;
        let buyer_base = buyer_balances.iter().find(|(a, _)| *a == 100).unwrap().1;

        assert_eq!(buyer_quote.avail(), 60000); // 40000 original + 20000 refund
        assert_eq!(buyer_quote.frozen(), 0);
        assert_eq!(buyer_base.avail(), 20); // Gained from trade

        // Verify seller state
        let seller_balances = ledger.get_user_balances(2).unwrap();
        let seller_base = seller_balances.iter().find(|(a, _)| *a == 100).unwrap().1;
        let seller_quote = seller_balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(seller_base.avail(), 30); // 20 original + 10 refund
        assert_eq!(seller_base.frozen(), 0);
        assert_eq!(seller_quote.avail(), 40000); // Gained from trade
    }

    #[test]
    fn test_listener_notification() {
        struct TestListener {
            commands: Arc<Mutex<Vec<LedgerCommand>>>,
        }

        impl LedgerListener for TestListener {
            fn on_command(&mut self, cmd: &LedgerCommand) -> Result<()> {
                self.commands.lock().unwrap().push(cmd.clone());
                Ok(())
            }
        }

        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        let commands = Arc::new(Mutex::new(Vec::new()));
        let listener = TestListener { commands: Arc::clone(&commands) };

        ledger.set_listener(Box::new(listener));

        // Apply some commands
        setup_user_with_balance(&mut ledger, 1, 100, 1000);
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 1,
                asset_id: 100,
                amount: 500,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Verify listener was called
        let recorded = commands.lock().unwrap();
        assert_eq!(recorded.len(), 2, "Listener should receive all commands");
    }

    #[test]
    fn test_empty_user_account() {
        let (ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // Query non-existent user
        let balance = ledger.get_balance(999, 100);
        assert_eq!(balance, 0);

        let balances = ledger.get_user_balances(999);
        assert!(balances.is_none());
    }

    #[test]
    fn test_version_tracking() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        setup_user_with_balance(&mut ledger, 1, 100, 1000);

        let balances = ledger.get_user_balances(1).unwrap();
        let initial_version = balances.iter().find(|(a, _)| *a == 100).unwrap().1.version();

        // Perform operation
        ledger
            .apply(&LedgerCommand::Withdraw {
                user_id: 1,
                asset_id: 100,
                amount: 100,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        let balances = ledger.get_user_balances(1).unwrap();
        let new_version = balances.iter().find(|(a, _)| *a == 100).unwrap().1.version();

        assert!(new_version > initial_version, "Version should increment on updates");
    }

    #[test]
    fn test_shadow_ledger_delta_merge() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // 1. Setup initial state
        setup_user_with_balance(&mut ledger, 1, 100, 1000); // User 1: 1000
        setup_user_with_balance(&mut ledger, 2, 100, 2000); // User 2: 2000
        ledger.flush().unwrap();

        // 2. Create a batch of commands
        let cmds = vec![
            // User 1: Lock 500
            LedgerCommand::Lock {
                user_id: 1,
                asset_id: 100,
                amount: 500,
                balance_after: 0,
                version: 0,
            },
            // User 2: Withdraw 1000
            LedgerCommand::Withdraw {
                user_id: 2,
                asset_id: 100,
                amount: 1000,
                balance_after: 0,
                version: 0,
            },
            // User 3 (New): Deposit 500
            LedgerCommand::Deposit {
                user_id: 3,
                asset_id: 100,
                amount: 500,
                balance_after: 0,
                version: 0,
            },
        ];

        // 3. Commit batch (triggers ShadowLedger delta merge)
        ledger.commit_batch(&cmds).unwrap();

        // 4. Verify Final State

        // User 1: 1000 - 500 (locked) = 500 avail, 500 frozen
        let u1 = ledger.get_user_balances(1).unwrap();
        let b1 = u1.iter().find(|(a, _)| *a == 100).unwrap().1;
        assert_eq!(b1.avail(), 500);
        assert_eq!(b1.frozen(), 500);

        // User 2: 2000 - 1000 (withdrawn) = 1000 avail, 0 frozen
        let u2 = ledger.get_user_balances(2).unwrap();
        let b2 = u2.iter().find(|(a, _)| *a == 100).unwrap().1;
        assert_eq!(b2.avail(), 1000);
        assert_eq!(b2.frozen(), 0);

        // User 3: 500 avail
        let u3 = ledger.get_user_balances(3).unwrap();
        let b3 = u3.iter().find(|(a, _)| *a == 100).unwrap().1;
        assert_eq!(b3.avail(), 500);
        assert_eq!(b3.frozen(), 0);

        // 5. Verify WAL persistence
        // We expect 2 initial deposits + 3 batch commands = 5 commands total
        assert_eq!(ledger.last_seq, 5);
    }

    #[test]
    fn test_shadow_ledger_delta_merge_trading() {
        let (mut ledger, _wal_dir, _snap_dir) = create_test_ledger();

        // 1. Setup initial state
        // Buyer (User 1): 1000 USDT (Asset 200)
        setup_user_with_balance(&mut ledger, 1, 200, 1000);
        // Seller (User 2): 10 BTC (Asset 100)
        setup_user_with_balance(&mut ledger, 2, 100, 10);
        ledger.flush().unwrap();

        // 2. Pre-lock funds (simulating order placement)
        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 1,
                asset_id: 200,
                amount: 1000,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        ledger
            .apply(&LedgerCommand::Lock {
                user_id: 2,
                asset_id: 100,
                amount: 10,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // 3. Create MatchExec command (Trade: 5 BTC @ 100 USDT = 500 USDT)
        let match_data = MatchExecData {
            trade_id: 1,
            buy_order_id: 101,
            sell_order_id: 102,
            buyer_user_id: 1,
            seller_user_id: 2,
            price: 100,
            quantity: 5,
            base_asset_id: 100,  // BTC
            quote_asset_id: 200, // USDT
            buyer_refund: 0,
            seller_refund: 0,
            match_seq: 1,
            output_sequence: 1,
            settled_at: 0,
            buyer_quote_version: 0,
            buyer_base_version: 0,
            seller_base_version: 0,
            seller_quote_version: 0,
            buyer_quote_balance_after: 0,
            buyer_base_balance_after: 0,
            seller_base_balance_after: 0,
            seller_quote_balance_after: 0,
        };

        let cmds = vec![LedgerCommand::MatchExec(match_data)];

        // 4. Commit batch (triggers ShadowLedger delta merge)
        ledger.commit_batch(&cmds).unwrap();

        // 5. Verify Final State

        // Buyer (User 1):
        // USDT: 1000 locked - 500 spent = 500 frozen
        // BTC: 0 + 5 gained = 5 avail
        let u1 = ledger.get_user_balances(1).unwrap();
        let b1_usdt = u1.iter().find(|(a, _)| *a == 200).unwrap().1;
        let b1_btc = u1.iter().find(|(a, _)| *a == 100).unwrap().1;

        assert_eq!(b1_usdt.frozen(), 500, "Buyer USDT frozen incorrect");
        assert_eq!(b1_usdt.avail(), 0, "Buyer USDT avail incorrect"); // All 1000 was locked
        assert_eq!(b1_btc.avail(), 5, "Buyer BTC avail incorrect");

        // Seller (User 2):
        // BTC: 10 locked - 5 spent = 5 frozen
        // USDT: 0 + 500 gained = 500 avail
        let u2 = ledger.get_user_balances(2).unwrap();
        let b2_btc = u2.iter().find(|(a, _)| *a == 100).unwrap().1;
        let b2_usdt = u2.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(b2_btc.frozen(), 5, "Seller BTC frozen incorrect");
        assert_eq!(b2_btc.avail(), 0, "Seller BTC avail incorrect"); // All 10 was locked
        assert_eq!(b2_usdt.avail(), 500, "Seller USDT avail incorrect");
    }
}
