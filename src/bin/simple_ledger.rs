use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use memmap2::{MmapMut, MmapOptions};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// ==========================================
// 1. 基础数据结构
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
// 2. 指令集 (Commands)
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
// 3. 高性能 WAL (Mmap + Bincode)
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

        // 如果是新文件，预分配 1GB
        if len == 0 {
            len = 1024 * 1024 * 1024; // 1GB
            file.set_len(len as u64)?;
        }

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // 扫描游标：找到数据结尾 (这里用简单的 0 检查，生产环境应用 Header+CRC)
        // 格式：[Length u32] [Payload...]
        let mut cursor = 0;
        while cursor + 4 < len {
            let len_bytes: [u8; 4] = mmap[cursor..cursor + 4].try_into().unwrap();
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len == 0 { break; } // 遇到 0 长度，说明后面是空白
            if cursor + 4 + payload_len > len { break; } // 数据不完整

            cursor += 4 + payload_len;
        }

        println!("WAL Opened. Resuming from offset: {}", cursor);

        Ok(Self { file, mmap, cursor, len })
    }

    #[inline(always)]
    pub fn append(&mut self, cmd: &LedgerCommand) -> Result<()> {
        // 1. 序列化 (Bincode 极快)
        // 计算大小 (Size Limit 设为 Infinite)
        let size = bincode::serialized_size(cmd)? as usize;

        // 2. 检查容量 & 自动扩容
        if self.cursor + 4 + size >= self.len {
            self.remap_grow()?;
        }

        // 3. 写入长度前缀 (u32)
        let len_bytes = (size as u32).to_le_bytes();
        self.mmap[self.cursor..self.cursor + 4].copy_from_slice(&len_bytes);
        self.cursor += 4;

        // 4. 写入 Payload
        bincode::serialize_into(&mut self.mmap[self.cursor..self.cursor + size], cmd)?;
        self.cursor += size;

        Ok(())
    }

    // 扩容逻辑：双倍扩容
    fn remap_grow(&mut self) -> Result<()> {
        self.mmap.flush()?;

        let new_len = self.len * 2;
        println!(">>> WAL Full. Extending from {} MB to {} MB...",
                 self.len / 1024 / 1024, new_len / 1024 / 1024);

        self.file.set_len(new_len as u64)?;

        // 重新映射
        self.mmap = unsafe { MmapOptions::new().map_mut(&self.file)? };
        self.len = new_len;

        Ok(())
    }

    // 恢复逻辑：读取所有历史指令
    pub fn replay(&self) -> Result<Vec<LedgerCommand>> {
        let mut commands = Vec::new();
        let mut cursor = 0;

        while cursor + 4 < self.len {
            let len_bytes: [u8; 4] = self.mmap[cursor..cursor + 4].try_into().unwrap();
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            if payload_len == 0 { break; }

            cursor += 4;
            let payload = &self.mmap[cursor..cursor + payload_len];
            let cmd: LedgerCommand = bincode::deserialize(payload)?;
            commands.push(cmd);

            cursor += payload_len;
        }
        Ok(commands)
    }
}

// ==========================================
// 4. 全局账本引擎 (The Ledger)
// ==========================================

pub struct GlobalLedger {
    accounts: FxHashMap<UserId, UserAccount>,
    wal: MmapWal, // 集成 WAL
    seq: u64,
}

impl GlobalLedger {
    // 初始化：打开 WAL 并恢复状态
    pub fn new(wal_path: &Path) -> Result<Self> {
        let wal = MmapWal::open(wal_path)?;

        let mut ledger = Self {
            accounts: FxHashMap::default(),
            wal,
            seq: 0,
        };

        // RECOVERY: 重放日志
        println!("Replaying WAL...");
        let history = ledger.wal.replay()?;
        let count = history.len();

        for cmd in history {
            // 这里我们调用内部 logic，跳过 wal append，防止重复写入
            ledger.apply_logic(&cmd)?;
            ledger.seq += 1;
        }
        println!("Recovered {} transactions. Current Seq: {}", count, ledger.seq);

        Ok(ledger)
    }

    // 对外接口：先写日志，再改内存
    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        // 1. Persistence (WAL)
        self.wal.append(cmd)?;
        self.seq += 1;

        // 2. Memory State
        self.apply_logic(cmd)?;

        Ok(())
    }

    // 纯内存逻辑 (拆分出来供 Replay 使用)
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

    pub fn print_user(&self, user_id: UserId) {
        if let Some(user) = self.accounts.get(&user_id) {
            println!("User {}: {:?}", user_id, user.assets);
        } else {
            println!("User {} not found", user_id);
        }
    }
}

// ==========================================
// 5. 主程序 (测试 WAL 持久化)
// ==========================================

fn main() -> Result<()> {
    let wal_path = Path::new("ledger.wal");

    // 1. Cleanup environment
    if wal_path.exists() { fs::remove_file(wal_path)?; }

    let mut ledger = GlobalLedger::new(wal_path)?;

    // ==========================================
    // Phase 1: Pre-warm (Initialize Data)
    // ==========================================
    // Use usize for easier modulo arithmetic
    let user_count = 10_000;
    println!(">>> SESSION 1: Pre-warming data (Initializing {} users)", user_count);

    for id in 0..user_count {
        ledger.apply(&LedgerCommand::Deposit {
            user_id: id as u64,
            asset: 1,
            amount: 1_000_000,
        })?;
    }
    println!("    Pre-warm complete.");

    // ==========================================
    // Phase 2: Performance Benchmark
    // ==========================================
    println!("\n>>> SESSION 2: Performance Benchmark (Persistence Enabled)");
    println!("    Scenario: 2M Ordered Operations (Round-Robin: Deposit -> Lock -> Unlock)");

    let total_ops = 2_000_000;
    let start = Instant::now();

    for i in 0..total_ops {
        let user_id = (i % user_count) as u64;

        // [CRITICAL FIX]: Use "Rounds" instead of random `i % 3`
        // Round 0: User 0..9999 all Deposit
        // Round 1: User 0..9999 all Lock
        // Round 2: User 0..9999 all Unlock
        // This guarantees logical correctness (you can't unlock before locking).
        let round = i / user_count;

        match round % 3 {
            0 => {
                // Round A: Deposit
                ledger.apply(&LedgerCommand::Deposit {
                    user_id,
                    asset: 1,
                    amount: 100,
                })?;
            }
            1 => {
                // Round B: Lock (Guaranteed to succeed because of Round A)
                ledger.apply(&LedgerCommand::Lock {
                    user_id,
                    asset: 1,
                    amount: 50,
                })?;
            }
            _ => {
                // Round C: Unlock (Guaranteed to succeed because of Round B)
                ledger.apply(&LedgerCommand::Unlock {
                    user_id,
                    asset: 1,
                    amount: 50,
                })?;
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

    Ok(())
}