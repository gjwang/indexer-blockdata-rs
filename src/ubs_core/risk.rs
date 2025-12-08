//! Risk models for UBSCore
//!
//! Different market types (Spot, Futures) have different risk rules.

use crate::user_account::UserAccount;
use super::order::{InternalOrder, Side};

/// Trait for different market types
pub trait RiskModel: Send + Sync {
    /// Check if account can afford the order
    fn can_trade(&self, account: &UserAccount, order: &InternalOrder) -> bool;

    /// Get the asset ID that will be debited
    fn get_debit_asset(&self, order: &InternalOrder, base_asset: u32, quote_asset: u32) -> u32;
}

/// Spot market: simple balance check
pub struct SpotRiskModel;

impl RiskModel for SpotRiskModel {
    fn can_trade(&self, account: &UserAccount, order: &InternalOrder) -> bool {
        let cost = order.calculate_cost();
        if cost == u64::MAX {
            return false;  // Overflow
        }

        // For spot: check available balance
        // Note: actual asset lookup depends on symbol config
        // This is a simplified check
        true  // Actual check done in UBSCore with asset info
    }

    fn get_debit_asset(&self, order: &InternalOrder, base_asset: u32, quote_asset: u32) -> u32 {
        match order.side {
            Side::Buy => quote_asset,   // Buy BTC/USDT: debit USDT
            Side::Sell => base_asset,   // Sell BTC/USDT: debit BTC
        }
    }
}

// Future: FuturesRiskModel with margin calculations
