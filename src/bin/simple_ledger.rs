use std::io::Write;
use std::time::Instant;

use anyhow::{bail, Result};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// ==========================================
// 1. 基础数据结构 (Data Structures)
// ==========================================

// 使用 u32 代表币种 (1=BTC, 2=ETH, 3=USDT)
// 字符串比较太慢，生产环境必须在启动时建立 String -> u32 映射
pub type AssetId = u32;
pub type UserId = u64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[repr(C)] // 保证内存布局紧凑
pub struct Balance {
    pub available: u64, // 可用资金
    pub frozen: u64,    // 冻结资金 (挂单中)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    pub user_id: UserId,
    // 优化：绝大多数用户持有币种 < 20 个。
    // 在小数据量下，Vec 的线性扫描 (O(N)) 远快于 HashMap (O(1) + Hash开销)
    // 且 Vec 内存连续，对 CPU Cache 极其友好。
    pub assets: Vec<(AssetId, Balance)>,
}

impl UserAccount {
    pub fn new(user_id: UserId) -> Self {
        Self { user_id, assets: Vec::with_capacity(8) }
    }

    // 获取余额的可变引用 (如果不存在则创建 0 余额)
    // 获取余额的可变引用 (如果不存在则创建 0 余额)
    #[inline(always)]
    pub fn get_balance_mut(&mut self, asset: AssetId) -> &mut Balance {
        // 1. 先尝试查找索引 (只读借用，用完即还)
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            // 2. 根据索引获取可变引用 (这是全新的借用，编译器能通过)
            return &mut self.assets[index].1;
        }

        // 3. 没找到，创建一个新的
        self.assets.push((asset, Balance::default()));
        &mut self.assets.last_mut().unwrap().1
    }
}


// ==========================================
// 2. 指令集 (Commands)
// ==========================================
// 这是账本唯一允许的操作集合。写 WAL 就是写这个 Enum。

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LedgerCommand {
    /// 充值 (外部 -> 可用)
    Deposit { user_id: UserId, asset: AssetId, amount: u64 },

    /// 提现 (可用 -> 外部)
    Withdraw { user_id: UserId, asset: AssetId, amount: u64 },

    /// 冻结 (可用 -> 冻结) - 下单前必做
    Lock { user_id: UserId, asset: AssetId, amount: u64 },

    /// 解冻 (冻结 -> 可用) - 撤单/流单
    Unlock { user_id: UserId, asset: AssetId, amount: u64 },

    /// 撮合结算 (冻结 -> 扣除, 获得 -> 可用)
    /// 注意：这里处理单边用户的结算
    TradeSettle {
        user_id: UserId,
        spend_asset: AssetId,
        spend_amount: u64, // 从冻结扣除
        gain_asset: AssetId,
        gain_amount: u64,   // 加到可用
    },
}

// ==========================================
// 3. 全局账本引擎 (The Ledger)
// ==========================================

pub struct GlobalLedger {
    // 核心内存数据
    accounts: FxHashMap<UserId, UserAccount>,
    // 简单的 WAL 模拟 (实际应使用之前的 Mmap WAL)
    seq: u64,
}

impl GlobalLedger {
    pub fn new() -> Self {
        Self {
            accounts: FxHashMap::default(),
            seq: 0,
        }
    }

    // 核心处理函数：应用指令
    // 返回 Result，如果余额不足会报错，状态回滚（实际上没改内存）
    pub fn apply(&mut self, cmd: &LedgerCommand) -> Result<()> {
        self.seq += 1;
        // 1. 在这里写入 WAL (self.wal.append(cmd))

        // 2. 应用内存逻辑
        match cmd {
            LedgerCommand::Deposit { user_id, asset, amount } => {
                let user = self.get_user(*user_id);
                let bal = user.get_balance_mut(*asset);
                // 严谨的溢出检查
                bal.available = bal.available.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }

            LedgerCommand::Withdraw { user_id, asset, amount } => {
                let user = self.get_or_error(*user_id)?;
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount { bail!("Insufficient available funds"); }
                bal.available -= amount;
            }

            LedgerCommand::Lock { user_id, asset, amount } => {
                let user = self.get_or_error(*user_id)?;
                let bal = user.get_balance_mut(*asset);
                if bal.available < *amount { bail!("Insufficient available funds to lock"); }

                bal.available -= amount;
                bal.frozen = bal.frozen.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }

            LedgerCommand::Unlock { user_id, asset, amount } => {
                let user = self.get_or_error(*user_id)?;
                let bal = user.get_balance_mut(*asset);
                if bal.frozen < *amount { bail!("Insufficient frozen funds to unlock"); }

                bal.frozen -= amount;
                bal.available = bal.available.checked_add(*amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }

            LedgerCommand::TradeSettle { user_id, spend_asset, spend_amount, gain_asset, gain_amount } => {
                let user = self.get_or_error(*user_id)?;

                // 1. 扣钱 (从冻结扣，因为下单时已经 Lock 了)
                let spend_bal = user.get_balance_mut(*spend_asset);
                if spend_bal.frozen < *spend_amount {
                    // 生产环境这是一个 Critical Bug (撮合引擎状态与账本不一致)
                    bail!("CRITICAL: Trade spend amount > frozen balance");
                }
                spend_bal.frozen -= spend_amount;

                // 2. 加钱 (进可用)
                let gain_bal = user.get_balance_mut(*gain_asset);
                gain_bal.available = gain_bal.available.checked_add(*gain_amount).ok_or(anyhow::anyhow!("Overflow"))?;
            }
        }
        Ok(())
    }

    // 辅助：获取用户，如果不存在则创建
    #[inline(always)]
    fn get_user(&mut self, user_id: UserId) -> &mut UserAccount {
        self.accounts.entry(user_id).or_insert_with(|| UserAccount::new(user_id))
    }

    // 辅助：获取用户，不存在则报错 (用于提现/下单)
    #[inline(always)]
    fn get_or_error(&mut self, user_id: UserId) -> Result<&mut UserAccount> {
        self.accounts.get_mut(&user_id).ok_or(anyhow::anyhow!("User not found"))
    }

    // 调试打印
    pub fn print_user(&self, user_id: UserId) {
        if let Some(user) = self.accounts.get(&user_id) {
            println!("User {}: {:?}", user_id, user.assets);
        } else {
            println!("User {} not found", user_id);
        }
    }
}

// ==========================================
// 4. 主程序测试
// ==========================================

fn main() -> Result<()> {
    let mut ledger = GlobalLedger::new();
    let start = Instant::now();

    // 模拟 3 个币种
    let btc = 1;
    let usdt = 2;

    println!(">>> 1. 充值 (Deposit)");
    // 给 User 100 充值 10000 USDT
    ledger.apply(&LedgerCommand::Deposit { user_id: 100, asset: usdt, amount: 10000 })?;
    ledger.print_user(100);

    println!("\n>>> 2. 下单冻结 (Lock)");
    // User 100 想买 BTC，冻结 5000 USDT
    ledger.apply(&LedgerCommand::Lock { user_id: 100, asset: usdt, amount: 5000 })?;
    ledger.print_user(100);

    println!("\n>>> 3. 撮合成交 (TradeSettle)");
    // 撮合成功：User 100 花费 5000 USDT，买入 0.1 BTC (假设 1 BTC = 50000 USDT)
    // 这里的 0.1 BTC 我们用 10000000 聪表示 (假设单位是聪)
    ledger.apply(&LedgerCommand::TradeSettle {
        user_id: 100,
        spend_asset: usdt,
        spend_amount: 5000,
        gain_asset: btc,
        gain_amount: 10_000_000,
    })?;
    ledger.print_user(100);

    println!("\n>>> 性能测试 (纯内存逻辑)");
    let total_ops = 10_000_000;
    let bench_start = Instant::now();

    for i in 0..total_ops {
        // 模拟一个高频的存取操作
        // 这里不做 unwrap，模拟真实环境处理 Error 的开销
        let _ = ledger.apply(&LedgerCommand::Deposit {
            user_id: 100,
            asset: usdt,
            amount: 1,
        });
    }

    let duration = bench_start.elapsed();
    println!("Processed {} ops in {:.2?}", total_ops, duration);
    println!("TPS: {:.0} ops/sec", total_ops as f64 / duration.as_secs_f64());

    Ok(())
}