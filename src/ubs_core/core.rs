//! UBSCore - the main balance authority
//!
//! Validates orders before they reach the Matching Engine.

use crate::user_account::{AssetId, UserAccount, UserId};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use super::debt::{DebtLedger, DebtReason, DebtRecord, EventType};
use super::dedup::DeduplicationGuard;
use super::error::RejectReason;
use super::fee::VipFeeTable;
use super::order::InternalOrder;
use super::risk::RiskModel;
use super::wal::{GroupCommitConfig, GroupCommitWal, WalEntry, WalEntryType};

const HIGH_WATER_MARK: usize = 10_000; // Backpressure threshold

/// Main UBSCore struct
pub struct UBSCore<R: RiskModel> {
    // State
    accounts: HashMap<UserId, UserAccount>,
    dedup_guard: DeduplicationGuard,
    debt_ledger: DebtLedger,
    pending_queue: VecDeque<InternalOrder>,

    // Fee configuration
    vip_configs: HashMap<UserId, u8>, // UserId → VIP level
    vip_table: VipFeeTable,

    // Logic
    risk_model: R,

    // Mode
    is_replay_mode: bool,

    // WAL for durability (optional - None for tests/embedded without persistence)
    wal: Option<Mutex<GroupCommitWal>>,
}

impl<R: RiskModel> UBSCore<R> {
    pub fn new(risk_model: R) -> Self {
        Self {
            accounts: HashMap::new(),
            dedup_guard: DeduplicationGuard::new(),
            debt_ledger: DebtLedger::new(),
            pending_queue: VecDeque::new(),
            vip_configs: HashMap::new(),
            vip_table: VipFeeTable::default(),
            risk_model,
            is_replay_mode: false,
            wal: None,
        }
    }

    /// Create UBSCore with WAL for durability
    pub fn with_wal(risk_model: R, wal_path: &Path) -> Result<Self, String> {
        let config = GroupCommitConfig::default();
        let wal = GroupCommitWal::open(wal_path, config)
            .map_err(|e| format!("Failed to open WAL: {:?}", e))?;

        Ok(Self {
            accounts: HashMap::new(),
            dedup_guard: DeduplicationGuard::new(),
            debt_ledger: DebtLedger::new(),
            pending_queue: VecDeque::new(),
            vip_configs: HashMap::new(),
            vip_table: VipFeeTable::default(),
            risk_model,
            is_replay_mode: false,
            wal: Some(Mutex::new(wal)),
        })
    }

    /// Set replay mode (use order timestamp instead of wall clock)
    pub fn set_replay_mode(&mut self, enabled: bool) {
        self.is_replay_mode = enabled;
    }

    /// Get current time (respects replay mode)
    fn current_time(&self, order: &InternalOrder) -> u64 {
        if self.is_replay_mode {
            order.timestamp_ms() // Use InternalOrder's method
        } else {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64
        }
    }

    /// Process incoming order (validation only, no WAL)
    pub fn process_order(&mut self, order: InternalOrder) -> Result<(), RejectReason> {
        // 0. BACKPRESSURE CHECK
        if self.pending_queue.len() > HIGH_WATER_MARK {
            return Err(RejectReason::SystemBusy);
        }

        // 1. Deduplication check
        let now = self.current_time(&order);
        self.dedup_guard.check_and_record(order.order_id, now)?;

        // 2. Get account
        let _account = self.accounts.get(&order.user_id).ok_or(RejectReason::AccountNotFound)?;

        // 3. Calculate cost internally (SECURITY)
        let cost = order.calculate_cost();
        if cost == u64::MAX {
            return Err(RejectReason::OrderCostOverflow);
        }

        // 4. Risk check
        // TODO: Full implementation with asset lookup

        // 5. Lock funds
        // TODO: Implement after we have symbol config

        Ok(())
    }

    /// Process order with WAL persistence
    /// "Once accepted, never lost" - order is persisted BEFORE returning Ok
    pub fn process_order_durable(&mut self, order: InternalOrder) -> Result<(), RejectReason> {
        // 1. Validate first (cheap, no I/O)
        self.process_order(order.clone())?;

        // 2. If valid, persist to WAL (fsync before returning)
        if let Some(wal_mutex) = &self.wal {
            let payload = bincode::serialize(&order)
                .map_err(|_| RejectReason::InternalError)?;

            let entry = WalEntry::new(WalEntryType::OrderLock, payload);

            let mut wal = wal_mutex.lock()
                .map_err(|_| RejectReason::InternalError)?;

            wal.append(&entry)
                .map_err(|_| RejectReason::InternalError)?;

            wal.flush()
                .map_err(|_| RejectReason::InternalError)?;
        }

        Ok(())
    }

    /// Check if user can withdraw
    /// BLOCKED if user has ANY debt
    pub fn can_withdraw(&self, user_id: UserId, asset_id: AssetId, amount: u64) -> bool {
        // 1. Block if user has ANY debt
        if self.debt_ledger.has_debt(user_id) {
            return false;
        }

        // 2. Check balance
        if let Some(account) = self.accounts.get(&user_id) {
            if let Some(balance) = account.get_balance(asset_id) {
                return balance.avail >= amount;
            }
        }

        false
    }

    /// Apply balance change with debt detection
    /// This is where ghost money is detected!
    pub fn apply_balance_delta(
        &mut self,
        user_id: UserId,
        asset_id: AssetId,
        delta: i64,
        event_type: EventType,
    ) -> u64 {
        let account = self.accounts.entry(user_id).or_insert_with(|| UserAccount::new(user_id));

        let balance = account.get_balance_mut(asset_id);

        let current = balance.avail as i64;
        let expected = current + delta;

        if expected >= 0 {
            // Normal case: no debt
            balance.avail = expected as u64;
            return balance.avail;
        }

        // Ghost money detected!
        balance.avail = 0;
        let shortfall = (-expected) as u64;

        // Derive reason from event type (no WAL change needed!)
        let reason = DebtReason::from_event_type(event_type);

        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;

        self.debt_ledger.add_debt(
            user_id,
            asset_id,
            DebtRecord { amount: shortfall, reason, created_at: now },
        );

        log::warn!(
            "GHOST_MONEY: user={} asset={} shortfall={} reason={:?}",
            user_id,
            asset_id,
            shortfall,
            reason
        );

        0
    }

    /// Deposit funds - pays debt first!
    pub fn on_deposit(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) {
        // First: Pay off any debt
        let remaining = self.debt_ledger.pay_debt(user_id, asset_id, amount);

        // Then: Deposit remaining to Balance
        if remaining > 0 {
            let account = self.accounts.entry(user_id).or_insert_with(|| UserAccount::new(user_id));
            let balance = account.get_balance_mut(asset_id);
            balance.avail += remaining;
        }
    }

    /// Calculate fee (simple: always quote asset)
    pub fn calculate_fee(&self, user_id: UserId, trade_value: u64, is_maker: bool) -> u64 {
        let vip_level = self.vip_configs.get(&user_id).copied().unwrap_or(0);
        let rate = self.vip_table.get_rate(vip_level, is_maker);
        (trade_value * rate) / 1_000_000
    }

    /// Get balance (for testing)
    pub fn get_balance(&self, user_id: UserId, asset_id: AssetId) -> Option<u64> {
        self.accounts.get(&user_id)?.get_balance(asset_id).map(|b| b.avail)
    }

    /// Check if user has debt
    pub fn has_debt(&self, user_id: UserId) -> bool {
        self.debt_ledger.has_debt(user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::risk::SpotRiskModel;
    use super::*;

    #[test]
    fn test_deposit_no_debt() {
        let mut core = UBSCore::new(SpotRiskModel);
        core.on_deposit(1, 1, 1000);
        assert_eq!(core.get_balance(1, 1), Some(1000));
    }

    #[test]
    fn test_apply_delta_positive() {
        let mut core = UBSCore::new(SpotRiskModel);
        core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
        assert_eq!(core.get_balance(1, 1), Some(1000));
    }

    #[test]
    fn test_apply_delta_ghost_money() {
        let mut core = UBSCore::new(SpotRiskModel);
        core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
        core.apply_balance_delta(1, 1, -5000, EventType::TradeSettle);

        // Balance clamped to 0
        assert_eq!(core.get_balance(1, 1), Some(0));
        // Debt created
        assert!(core.has_debt(1));
    }

    #[test]
    fn test_can_withdraw_blocked_by_debt() {
        let mut core = UBSCore::new(SpotRiskModel);
        core.apply_balance_delta(1, 1, 10000, EventType::Deposit);
        // Create debt in another asset
        core.apply_balance_delta(1, 2, -5000, EventType::TradeSettle);
        // Cannot withdraw from asset 1 because user has debt in asset 2
        assert!(!core.can_withdraw(1, 1, 100));
    }
}
