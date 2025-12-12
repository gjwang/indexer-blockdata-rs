//! Atomic Transfer Adapter
//!
//! Uses TigerBeetle's native atomic transfers for guaranteed consistency.
//! This adapter provides true atomicity - either the entire transfer succeeds
//! or it fails completely with no partial state.

use std::sync::Arc;
use async_trait::async_trait;

use tigerbeetle_unofficial::Client;

use crate::transfer::types::{OpResult, RequestId, ServiceId};
use crate::ubs_core::tigerbeetle::{
    tb_account_id, ensure_account, atomic_transfer as tb_atomic_transfer,
    create_pending_transfer, post_pending_transfer, void_pending_transfer,
    TRADING_LEDGER,
};

/// Atomic Transfer Adapter using TigerBeetle's native atomic guarantees
///
/// This adapter is used when both source and target are in TigerBeetle (Funding accounts).
/// It provides true atomic transfers where:
/// 1. Funds are frozen (pending transfer)
/// 2. Transfer is committed atomically
/// 3. If commit fails, funds are automatically unfrozen
pub struct AtomicTransferAdapter {
    client: Arc<Client>,
}

impl AtomicTransferAdapter {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }

    /// Execute an atomic transfer between two Funding accounts
    ///
    /// This is a single operation that either succeeds completely or fails completely.
    /// TigerBeetle guarantees atomicity at the database level.
    pub async fn execute_atomic_transfer(
        &self,
        req_id: RequestId,
        from_user_id: u64,
        to_user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        let transfer_id = req_id.as_u128();
        let from_account = tb_account_id(from_user_id, asset_id);
        let to_account = tb_account_id(to_user_id, asset_id);

        log::info!(
            "Executing atomic transfer: {} from {} to {} amount={}",
            req_id, from_user_id, to_user_id, amount
        );

        // Ensure both accounts exist
        if let Err(e) = ensure_account(&self.client, from_account, TRADING_LEDGER, 1).await {
            log::error!("Failed to ensure source account: {}", e);
            return OpResult::Failed(format!("Source account error: {}", e));
        }
        if let Err(e) = ensure_account(&self.client, to_account, TRADING_LEDGER, 1).await {
            log::error!("Failed to ensure destination account: {}", e);
            return OpResult::Failed(format!("Destination account error: {}", e));
        }

        // Execute atomic transfer
        // Code 10 = Internal Transfer
        match tb_atomic_transfer(
            &self.client,
            transfer_id,
            from_account,
            to_account,
            amount,
            TRADING_LEDGER,
            10, // Internal transfer code
        ).await {
            Ok(_) => {
                log::info!("Atomic transfer completed: {}", req_id);
                OpResult::Success
            }
            Err(e) => {
                log::error!("Atomic transfer failed: {} - {}", req_id, e);
                // Check if it's a balance issue
                if e.contains("ExceedsCredits") || e.contains("exceeds") {
                    OpResult::Failed("Insufficient balance".to_string())
                } else {
                    OpResult::Pending // Retry on other errors
                }
            }
        }
    }

    /// Create a pending (frozen) transfer for 2-phase commit
    ///
    /// Use this when you need the ability to void the transfer before commit.
    pub async fn create_pending(
        &self,
        req_id: RequestId,
        from_user_id: u64,
        to_user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        let transfer_id = req_id.as_u128();
        let from_account = tb_account_id(from_user_id, asset_id);
        let to_account = tb_account_id(to_user_id, asset_id);

        // Ensure accounts exist
        let _ = ensure_account(&self.client, from_account, TRADING_LEDGER, 1).await;
        let _ = ensure_account(&self.client, to_account, TRADING_LEDGER, 1).await;

        match create_pending_transfer(
            &self.client,
            transfer_id,
            from_account,
            to_account,
            amount,
            TRADING_LEDGER,
            10,
        ).await {
            Ok(_) => OpResult::Success,
            Err(e) => {
                if e.contains("ExceedsCredits") {
                    OpResult::Failed("Insufficient balance".to_string())
                } else {
                    OpResult::Pending
                }
            }
        }
    }

    /// Commit a pending transfer
    pub async fn commit_pending(&self, req_id: RequestId) -> OpResult {
        let pending_id = req_id.as_u128();
        match post_pending_transfer(&self.client, pending_id).await {
            Ok(_) => OpResult::Success,
            Err(_) => OpResult::Pending,
        }
    }

    /// Void (rollback) a pending transfer
    pub async fn void_pending(&self, req_id: RequestId) -> OpResult {
        let pending_id = req_id.as_u128();
        match void_pending_transfer(&self.client, pending_id).await {
            Ok(_) => OpResult::Success,
            Err(_) => OpResult::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_id_generation() {
        let account = tb_account_id(1001, 1);
        assert!(account > 0);

        // Different user/asset = different account
        let account2 = tb_account_id(1001, 2);
        assert_ne!(account, account2);
    }
}
