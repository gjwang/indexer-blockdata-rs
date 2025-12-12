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
    tb_account_id, ensure_account, TRADING_LEDGER, HOLDING_ACCOUNT_ID_PREFIX,
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
                log::info!("Trading withdraw (pending) succeeded: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Trading withdraw error: {} - {:?}", req_id, e);
                OpResult::Pending
            }
        }
    }

    /// Deposit to trading: Transfer from holding to user account
    async fn deposit(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::info!("TbTradingAdapter::deposit({}, user={}, asset={}, amount={})", req_id, user_id, asset_id, amount);

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

// ============================================================================
// UBSCore-backed adapter (via Aeron - for production)
// ============================================================================

#[cfg(feature = "aeron")]
mod ubs_adapter {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;
    use crate::ubs_core::comm::{AeronConfig, UbsGatewayClient, reason_codes};

    /// UBSCore-backed Trading Adapter
    /// Communicates with UBSCore via Aeron for real Spot/Trading account operations.
    ///
    /// This is the production adapter that integrates with the trading system:
    /// - Deposit to Trading: Increases user's available balance in UBSCore
    /// - Withdraw from Trading: Decreases user's available balance in UBSCore
    pub struct UbsTradingAdapter {
        client: Arc<AsyncMutex<UbsGatewayClient>>,
        timeout_ms: u64,
    }

    impl UbsTradingAdapter {
        /// Create a new UBS Trading Adapter with default Aeron config
        pub fn new() -> Result<Self, String> {
            Self::with_config(AeronConfig::default(), 5000)
        }

        /// Create with custom config and timeout
        pub fn with_config(config: AeronConfig, timeout_ms: u64) -> Result<Self, String> {
            let mut client = UbsGatewayClient::new(config);
            client.connect().map_err(|e| format!("Failed to connect to UBSCore: {:?}", e))?;

            log::info!("[UBS_TRADING] Connected to UBSCore via Aeron");

            Ok(Self {
                client: Arc::new(AsyncMutex::new(client)),
                timeout_ms,
            })
        }
    }

    #[async_trait]
    impl ServiceAdapter for UbsTradingAdapter {
        /// Withdraw from trading: Send Withdraw message to UBSCore
        /// This decreases the user's available balance in the Trading/Spot account
        async fn withdraw(
            &self,
            req_id: RequestId,
            user_id: u64,
            asset_id: u32,
            amount: u64,
        ) -> OpResult {
            log::info!("UbsTradingAdapter::withdraw({}, user={}, asset={}, amount={})",
                req_id, user_id, asset_id, amount);

            let client = self.client.lock().await;
            match client.send_withdraw(req_id.as_u64(), user_id, asset_id, amount, self.timeout_ms).await {
                Ok(resp) if resp.is_accepted() => {
                    log::info!("[UBS_TRADING] Withdraw {} accepted", req_id);
                    OpResult::Success
                }
                Ok(resp) => {
                    let reason = match resp.reason_code {
                        code if code == reason_codes::INSUFFICIENT_BALANCE => "insufficient balance",
                        code if code == reason_codes::ACCOUNT_NOT_FOUND => "account not found",
                        _ => "unknown error",
                    };
                    log::warn!("[UBS_TRADING] Withdraw {} rejected: {}", req_id, reason);
                    OpResult::Failed(format!("UBSCore rejected: {}", reason))
                }
                Err(e) => {
                    log::error!("[UBS_TRADING] Withdraw {} failed: {:?}", req_id, e);
                    OpResult::Pending
                }
            }
        }

        /// Deposit to trading: Send Deposit message to UBSCore
        /// This increases the user's available balance in the Trading/Spot account
        async fn deposit(
            &self,
            req_id: RequestId,
            user_id: u64,
            asset_id: u32,
            amount: u64,
        ) -> OpResult {
            log::info!("UbsTradingAdapter::deposit({}, user={}, asset={}, amount={})",
                req_id, user_id, asset_id, amount);

            let client = self.client.lock().await;
            match client.send_deposit(req_id.as_u64(), user_id, asset_id, amount, self.timeout_ms).await {
                Ok(resp) if resp.is_accepted() => {
                    log::info!("[UBS_TRADING] Deposit {} accepted", req_id);
                    OpResult::Success
                }
                Ok(resp) => {
                    let reason = match resp.reason_code {
                        code if code == reason_codes::INSUFFICIENT_BALANCE => "insufficient balance",
                        code if code == reason_codes::ACCOUNT_NOT_FOUND => "account not found",
                        _ => "unknown error",
                    };
                    log::warn!("[UBS_TRADING] Deposit {} rejected: {}", req_id, reason);
                    OpResult::Failed(format!("UBSCore rejected: {}", reason))
                }
                Err(e) => {
                    log::error!("[UBS_TRADING] Deposit {} failed: {:?}", req_id, e);
                    OpResult::Pending
                }
            }
        }

        /// Commit: No-op for UBSCore (already committed on success)
        async fn commit(&self, req_id: RequestId) -> OpResult {
            log::debug!("UbsTradingAdapter::commit({}) - no-op", req_id);
            OpResult::Success
        }

        /// Rollback: Not implemented for UBSCore
        async fn rollback(&self, req_id: RequestId) -> OpResult {
            log::warn!("UbsTradingAdapter::rollback({}) - not implemented", req_id);
            OpResult::Success
        }

        async fn query(&self, _req_id: RequestId) -> OpResult {
            OpResult::Pending
        }

        fn name(&self) -> &str {
            "trading-ubs"
        }
    }
}

#[cfg(feature = "aeron")]
pub use ubs_adapter::UbsTradingAdapter;
