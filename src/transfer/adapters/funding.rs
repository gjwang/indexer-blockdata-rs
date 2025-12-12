//! Funding service adapter
//!
//! Implements freeze/commit pattern for withdrawals.

use async_trait::async_trait;

use crate::transfer::types::{OpResult, RequestId};
use super::traits::ServiceAdapter;

/// Funding service adapter (placeholder)
///
/// Uses freeze/commit pattern:
/// - withdraw: Freezes funds (available -= amount, frozen += amount)
/// - commit: Releases frozen (frozen -= amount)
/// - rollback: Unfreezes (frozen -= amount, available += amount)
pub struct FundingAdapter {
    // TODO: Add TigerBeetle client for balance operations
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
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!(
            "FundingAdapter::withdraw({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );

        // Placeholder: Return Success for now
        OpResult::Success
    }

    async fn deposit(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!(
            "FundingAdapter::deposit({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );

        OpResult::Success
    }

    async fn commit(&self, req_id: RequestId) -> OpResult {
        log::info!("FundingAdapter::commit({})", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: RequestId) -> OpResult {
        log::info!("FundingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: RequestId) -> OpResult {
        log::info!("FundingAdapter::query({})", req_id);
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "funding"
    }
}

// ============================================================================
// TigerBeetle-backed adapter
// ============================================================================

use std::sync::Arc;
use tigerbeetle_unofficial::{Client, Transfer};
use tigerbeetle_unofficial::transfer::Flags as TransferFlags;

use crate::ubs_core::tigerbeetle::{
    tb_account_id, ensure_account, TRADING_LEDGER, HOLDING_ACCOUNT_ID_PREFIX,
};

/// TigerBeetle-backed Funding Adapter
/// Uses pending transfers with frozen balance - no omnibus account needed
pub struct TbFundingAdapter {
    client: Arc<Client>,
}

impl TbFundingAdapter {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }

    fn transfer_id(req_id: RequestId) -> u128 {
        req_id.as_u128()
    }
}

#[async_trait]
impl ServiceAdapter for TbFundingAdapter {
    /// Withdraw from funding: Create PENDING transfer that freezes funds
    async fn withdraw(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbFundingAdapter::withdraw({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

        let user_account = tb_account_id(user_id, asset_id);
        let holding_account = tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, asset_id);

        if let Err(e) = ensure_account(&self.client, user_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure user account: {}", e);
            return OpResult::Pending;
        }
        if let Err(e) = ensure_account(&self.client, holding_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure holding account: {}", e);
            return OpResult::Pending;
        }

        // Create PENDING transfer: User -> Holding
        // Funds are frozen in user's debits_pending until committed/rolled back
        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_debit_account_id(user_account)
            .with_credit_account_id(holding_account)
            .with_amount(amount as u128)
            .with_ledger(TRADING_LEDGER)
            .with_code(1)
            .with_flags(TransferFlags::PENDING);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Funding withdraw (pending) succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Funding withdraw error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Deposit to funding: Transfer from holding to user account
    async fn deposit(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbFundingAdapter::deposit({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

        let user_account = tb_account_id(user_id, asset_id);
        let holding_account = tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, asset_id);

        if let Err(e) = ensure_account(&self.client, user_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure user account: {}", e);
            return OpResult::Pending;
        }
        if let Err(e) = ensure_account(&self.client, holding_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure holding account: {}", e);
            return OpResult::Pending;
        }

        // Transfer: Holding -> User
        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_debit_account_id(holding_account)
            .with_credit_account_id(user_account)
            .with_amount(amount as u128)
            .with_ledger(TRADING_LEDGER)
            .with_code(2); // Different code for deposit

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Funding deposit succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Funding deposit error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Commit: Post the pending transfer (release frozen funds)
    async fn commit(&self, req_id: RequestId) -> OpResult {
        log::info!("TbFundingAdapter::commit({})", req_id);

        let pending_id = Self::transfer_id(req_id);
        let post_id = pending_id.wrapping_add(1);

        let transfer = Transfer::new(post_id)
            .with_pending_id(pending_id)
            .with_flags(TransferFlags::POST_PENDING_TRANSFER);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Funding commit succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Funding commit error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Rollback: Void the pending transfer (unfreeze funds)
    async fn rollback(&self, req_id: RequestId) -> OpResult {
        log::info!("TbFundingAdapter::rollback({})", req_id);

        let pending_id = Self::transfer_id(req_id);
        let void_id = pending_id.wrapping_add(2);

        let transfer = Transfer::new(void_id)
            .with_pending_id(pending_id)
            .with_flags(TransferFlags::VOID_PENDING_TRANSFER);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Funding rollback succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Funding rollback error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    async fn query(&self, _req_id: RequestId) -> OpResult {
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "funding-tb"
    }
}
