//! Funding service adapter
//!
//! Implements freeze/commit pattern for withdrawals.

use async_trait::async_trait;
use uuid::Uuid;

use crate::transfer::types::OpResult;
use super::traits::ServiceAdapter;

/// Funding service adapter
///
/// Uses freeze/commit pattern:
/// - withdraw: Freezes funds (available -= amount, frozen += amount)
/// - commit: Releases frozen (frozen -= amount)
/// - rollback: Unfreezes (frozen -= amount, available += amount)
pub struct FundingAdapter {
    // TODO: Add TigerBeetle client for balance operations
    // TODO: Add DB for idempotency tracking
}

impl FundingAdapter {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for FundingAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ServiceAdapter for FundingAdapter {
    async fn withdraw(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        // TODO: Implement freeze logic with TigerBeetle
        // 1. Check idempotency (have we processed this req_id?)
        // 2. Lock funds: available -= amount, frozen += amount
        // 3. Return Success (immediate for Funding)
        log::info!(
            "FundingAdapter::withdraw({}, user={}, asset={}, amount={})",
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
        // TODO: Implement direct credit with TigerBeetle
        // 1. Check idempotency
        // 2. Credit funds: available += amount
        // 3. Return Success
        log::info!(
            "FundingAdapter::deposit({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );

        OpResult::Success
    }

    async fn commit(&self, req_id: Uuid) -> OpResult {
        // TODO: Finalize freeze with TigerBeetle
        // 1. Delete frozen funds entry
        // 2. Return Success
        log::info!("FundingAdapter::commit({})", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        // TODO: Unfreeze with TigerBeetle
        // 1. frozen -= amount, available += amount
        // 2. Return Success
        log::info!("FundingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
        // TODO: Check operation status from DB
        log::info!("FundingAdapter::query({})", req_id);
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "funding"
    }
}
