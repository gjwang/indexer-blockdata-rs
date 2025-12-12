//! Trading service adapter
//!
//! Implements direct debit/credit pattern.

use async_trait::async_trait;
use uuid::Uuid;

use crate::transfer::types::OpResult;
use super::traits::ServiceAdapter;

/// Trading service adapter
///
/// Uses direct debit/credit pattern:
/// - withdraw: Direct debit (available -= amount)
/// - commit: No-op (already debited)
/// - rollback: Credit back (available += amount)
pub struct TradingAdapter {
    // TODO: Add UBSCore client for balance operations
    // TODO: Add Aeron publisher for async operations
}

impl TradingAdapter {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for TradingAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ServiceAdapter for TradingAdapter {
    async fn withdraw(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        // TODO: Implement direct debit via UBSCore
        // 1. Check idempotency
        // 2. Debit funds directly: available -= amount
        // 3. Return Success or Failed(insufficient)
        log::info!(
            "TradingAdapter::withdraw({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );

        // Placeholder: Return Success for now
        OpResult::Success
    }

    async fn deposit(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        // TODO: Implement direct credit via UBSCore
        // 1. Check idempotency
        // 2. Credit funds: available += amount
        // 3. Return Success
        log::info!(
            "TradingAdapter::deposit({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );

        OpResult::Success
    }

    async fn commit(&self, req_id: Uuid) -> OpResult {
        // Trading has no freeze stage, commit is no-op
        log::debug!("TradingAdapter::commit({}) - no-op", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        // TODO: Credit back via UBSCore
        // 1. Re-credit the debited amount
        // 2. Return Success
        log::info!("TradingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
        // TODO: Check operation status
        log::info!("TradingAdapter::query({})", req_id);
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "trading"
    }
}
