//! Debt ledger for ghost money handling
//!
//! DebtLedger is DERIVED state - not persisted to WAL separately.
//! DebtReason is derived from EventType (already in WAL).

use crate::user_account::{AssetId, UserId};
use std::collections::HashMap;

/// Event types that can cause balance changes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventType {
    // Can cause debt (ghost money)
    TradeSettle,
    OrderFee,
    Liquidation,
    StaleSpeculative,

    // Cannot cause debt (increases balance)
    Deposit,
    OrderUnlock,
    Withdraw,
}

/// Reason for debt - derived from EventType
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DebtReason {
    GhostMoney,       // TradeSettle with insufficient funds
    FeeUnpaid,        // OrderFee with insufficient funds
    Liquidation,      // Forced position close deficit
    StaleSpeculative, // Hot path credit expired
    SystemError,      // Should NEVER happen (defensive)
}

impl DebtReason {
    /// Derive debt reason from event type (exhaustive match)
    pub fn from_event_type(event_type: EventType) -> Self {
        match event_type {
            EventType::TradeSettle => DebtReason::GhostMoney,
            EventType::OrderFee => DebtReason::FeeUnpaid,
            EventType::Liquidation => DebtReason::Liquidation,
            EventType::StaleSpeculative => DebtReason::StaleSpeculative,

            // These should NEVER cause debt
            EventType::Deposit | EventType::OrderUnlock | EventType::Withdraw => {
                log::error!("IMPOSSIBLE: {:?} caused debt!", event_type);
                DebtReason::SystemError
            }
        }
    }
}

/// Record of a debt
#[derive(Debug, Clone)]
pub struct DebtRecord {
    pub amount: u64,
    pub created_at: u64,
    pub reason: DebtReason,
}

/// Ledger tracking user debts (ghost money)
pub struct DebtLedger {
    debts: HashMap<(UserId, AssetId), DebtRecord>,
}

impl DebtLedger {
    pub fn new() -> Self {
        Self { debts: HashMap::new() }
    }

    /// Check if user has ANY debt
    pub fn has_debt(&self, user_id: UserId) -> bool {
        self.debts.keys().any(|(uid, _)| *uid == user_id)
    }

    /// Get debt for specific asset
    pub fn get_debt(&self, user_id: UserId, asset_id: AssetId) -> Option<&DebtRecord> {
        self.debts.get(&(user_id, asset_id))
    }

    /// Add or increase debt
    pub fn add_debt(&mut self, user_id: UserId, asset_id: AssetId, record: DebtRecord) {
        log::warn!(
            "DEBT_CREATED: user={} asset={} amount={} reason={:?}",
            user_id,
            asset_id,
            record.amount,
            record.reason
        );

        self.debts
            .entry((user_id, asset_id))
            .and_modify(|existing| existing.amount += record.amount)
            .or_insert(record);
    }

    /// Pay off debt, returns remaining amount after debt payment
    pub fn pay_debt(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) -> u64 {
        if let Some(debt) = self.debts.get_mut(&(user_id, asset_id)) {
            let pay_off = amount.min(debt.amount);
            debt.amount -= pay_off;

            if debt.amount == 0 {
                self.debts.remove(&(user_id, asset_id));
            }

            amount - pay_off
        } else {
            amount
        }
    }

    /// Get total debt for a user across all assets
    pub fn total_debt(&self, user_id: UserId) -> u64 {
        self.debts
            .iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, record)| record.amount)
            .sum()
    }
}

impl Default for DebtLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debt_reason_from_trade_settle() {
        let reason = DebtReason::from_event_type(EventType::TradeSettle);
        assert_eq!(reason, DebtReason::GhostMoney);
    }

    #[test]
    fn test_debt_reason_from_fee() {
        let reason = DebtReason::from_event_type(EventType::OrderFee);
        assert_eq!(reason, DebtReason::FeeUnpaid);
    }

    #[test]
    fn test_add_debt() {
        let mut ledger = DebtLedger::new();
        ledger.add_debt(
            1,
            1,
            DebtRecord { amount: 1000, created_at: 12345, reason: DebtReason::GhostMoney },
        );
        assert!(ledger.has_debt(1));
        assert_eq!(ledger.get_debt(1, 1).unwrap().amount, 1000);
    }

    #[test]
    fn test_pay_debt_partial() {
        let mut ledger = DebtLedger::new();
        ledger.add_debt(
            1,
            1,
            DebtRecord { amount: 1000, created_at: 12345, reason: DebtReason::GhostMoney },
        );

        let remaining = ledger.pay_debt(1, 1, 600);
        assert_eq!(remaining, 0); // All used to pay debt
        assert_eq!(ledger.get_debt(1, 1).unwrap().amount, 400);
    }

    #[test]
    fn test_pay_debt_full() {
        let mut ledger = DebtLedger::new();
        ledger.add_debt(
            1,
            1,
            DebtRecord { amount: 500, created_at: 12345, reason: DebtReason::GhostMoney },
        );

        let remaining = ledger.pay_debt(1, 1, 1000);
        assert_eq!(remaining, 500); // 500 left after paying debt
        assert!(!ledger.has_debt(1));
    }
}
