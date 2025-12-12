//! Trading service adapter
//!
//! Implements direct debit/credit pattern.

use async_trait::async_trait;

use crate::transfer::types::{OpResult, RequestId};
use super::traits::ServiceAdapter;

/// Trading service adapter (placeholder)
///
/// Uses direct debit/credit pattern:
/// - withdraw: Direct debit (available -= amount)
/// - commit: No-op (already debited)
/// - rollback: Credit back (available += amount)
pub struct TradingAdapter {
    // TODO: Add UBSCore client for balance operations
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
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!(
            "TradingAdapter::withdraw({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );
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
            "TradingAdapter::deposit({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount
        );
        OpResult::Success
    }

    async fn commit(&self, req_id: RequestId) -> OpResult {
        log::debug!("TradingAdapter::commit({}) - no-op", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: RequestId) -> OpResult {
        log::info!("TradingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: RequestId) -> OpResult {
        log::info!("TradingAdapter::query({})", req_id);
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "trading"
    }
}

// ============================================================================
// TigerBeetle-backed adapter
// ============================================================================

use std::sync::Arc;
use tigerbeetle_unofficial::{Client, Transfer};
use tigerbeetle_unofficial::transfer::Flags as TransferFlags;

use crate::ubs_core::tigerbeetle::{
    tb_account_id, ensure_account, TRADING_LEDGER,
};

/// TigerBeetle-backed Trading Adapter
/// Uses pending transfers with frozen balance - no omnibus account needed
pub struct TbTradingAdapter {
    client: Arc<Client>,
}

impl TbTradingAdapter {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }

    fn transfer_id(req_id: RequestId) -> u128 {
        req_id.as_u128() | (1u128 << 127) // Different namespace than funding
    }
}

#[async_trait]
impl ServiceAdapter for TbTradingAdapter {
    /// Withdraw from trading: Create PENDING transfer that freezes funds
    async fn withdraw(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbTradingAdapter::withdraw({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

        let user_account = tb_account_id(user_id, asset_id);

        if let Err(e) = ensure_account(&self.client, user_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure user account: {}", e);
            return OpResult::Pending;
        }

        // Create PENDING transfer - funds are frozen in user's debits_pending
        // We use user_account as both debit and credit with PENDING flag
        // This freezes the amount without moving it anywhere
        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_debit_account_id(user_account)
            .with_credit_account_id(user_account) // Same account - TB allows this with PENDING
            .with_amount(amount as u128)
            .with_ledger(TRADING_LEDGER)
            .with_code(1)
            .with_flags(TransferFlags::PENDING);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Trading withdraw (pending) succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Trading withdraw error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Deposit to trading: Direct transfer to user account
    async fn deposit(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbTradingAdapter::deposit({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

        let user_account = tb_account_id(user_id, asset_id);

        if let Err(e) = ensure_account(&self.client, user_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure user account: {}", e);
            return OpResult::Pending;
        }

        // For deposit, we credit the user directly
        // This is a self-transfer that increases the balance
        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_debit_account_id(user_account)
            .with_credit_account_id(user_account)
            .with_amount(amount as u128)
            .with_ledger(TRADING_LEDGER)
            .with_code(2); // Different code for deposit

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Trading deposit succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Trading deposit error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Commit: Post the pending transfer (release frozen funds)
    async fn commit(&self, req_id: RequestId) -> OpResult {
        log::info!("TbTradingAdapter::commit({})", req_id);

        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_flags(TransferFlags::POST_PENDING_TRANSFER);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Trading commit succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Trading commit error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Rollback: Void the pending transfer (unfreeze funds)
    async fn rollback(&self, req_id: RequestId) -> OpResult {
        log::info!("TbTradingAdapter::rollback({})", req_id);

        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_flags(TransferFlags::VOID_PENDING_TRANSFER);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Trading rollback succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Trading rollback error: {} - {:?}", req_id, e);
                OpResult::Failed(format!("Rollback failed: {:?}", e))
            }
        }
    }

    async fn query(&self, _req_id: RequestId) -> OpResult {
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "trading-tb"
    }
}
