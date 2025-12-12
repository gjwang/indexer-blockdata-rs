//! Trading service adapter
//!
//! Implements direct debit/credit pattern.

use async_trait::async_trait;
use uuid::Uuid;

use crate::transfer::types::OpResult;
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
        req_id: Uuid,
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
        req_id: Uuid,
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

    async fn commit(&self, req_id: Uuid) -> OpResult {
        log::debug!("TradingAdapter::commit({}) - no-op", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        log::info!("TradingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
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

use crate::ubs_core::tigerbeetle::{
    tb_account_id, ensure_account,
    EXCHANGE_OMNIBUS_ID_PREFIX, TRADING_LEDGER,
};

/// TigerBeetle-backed Trading Adapter
pub struct TbTradingAdapter {
    client: Arc<Client>,
}

impl TbTradingAdapter {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }

    fn transfer_id(req_id: Uuid) -> u128 {
        req_id.as_u128() | (1u128 << 127) // Different namespace than funding
    }
}

#[async_trait]
impl ServiceAdapter for TbTradingAdapter {
    async fn withdraw(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbTradingAdapter::withdraw({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

        let user_account = tb_account_id(user_id, asset_id);
        let omnibus_account = tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id);

        if let Err(e) = ensure_account(&self.client, user_account, TRADING_LEDGER, 1).await {
            log::warn!("Failed to ensure user account: {}", e);
            return OpResult::Pending;
        }

        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_debit_account_id(user_account)
            .with_credit_account_id(omnibus_account)
            .with_amount(amount as u128)
            .with_ledger(TRADING_LEDGER)
            .with_code(1);

        match self.client.create_transfers(vec![transfer]).await {
            Ok(_) => {
                log::info!("Trading withdraw succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Trading withdraw error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    async fn deposit(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbTradingAdapter::deposit({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

        let user_account = tb_account_id(user_id, asset_id);
        let omnibus_account = tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id);

        let transfer = Transfer::new(Self::transfer_id(req_id))
            .with_debit_account_id(omnibus_account)
            .with_credit_account_id(user_account)
            .with_amount(amount as u128)
            .with_ledger(TRADING_LEDGER)
            .with_code(1);

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

    async fn commit(&self, _req_id: Uuid) -> OpResult {
        log::debug!("TbTradingAdapter::commit - no-op for trading");
        OpResult::Success
    }

    async fn rollback(&self, _req_id: Uuid) -> OpResult {
        log::warn!("TbTradingAdapter::rollback - not supported without state lookup");
        OpResult::Failed("Rollback not supported for trading".to_string())
    }

    async fn query(&self, _req_id: Uuid) -> OpResult {
        OpResult::Pending
    }

    fn name(&self) -> &str {
        "trading-tb"
    }
}
