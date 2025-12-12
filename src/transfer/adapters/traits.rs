//! Service adapter trait
//!
//! Defines the interface for interacting with different services (Funding, Trading).

use async_trait::async_trait;

use crate::transfer::types::{OpResult, RequestId};

/// Service adapter trait - implemented by each service
///
/// All operations MUST be idempotent: calling with the same req_id
/// should return the same result without side effects.
#[async_trait]
pub trait ServiceAdapter: Send + Sync {
    /// Withdraw/debit funds from user account
    ///
    /// For Funding: Freezes funds (available -= amount, frozen += amount)
    /// For Trading: Direct debit (available -= amount)
    ///
    /// Returns:
    /// - Success: Funds successfully withdrawn/frozen
    /// - Failed: Business failure (e.g., insufficient funds)
    /// - Pending: Technical error or in-flight (retry)
    async fn withdraw(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult;

    /// Deposit/credit funds to user account
    ///
    /// For both services: Direct credit (available += amount)
    ///
    /// Returns:
    /// - Success: Funds successfully credited
    /// - Failed: Business failure (rare for deposits)
    /// - Pending: Technical error or in-flight (retry)
    async fn deposit(
        &self,
        req_id: RequestId,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult;

    /// Finalize a pending withdraw (for services with freeze stage)
    ///
    /// For Funding: Releases frozen funds (frozen -= amount)
    /// For Trading: No-op (already debited directly)
    async fn commit(&self, req_id: RequestId) -> OpResult;

    /// Rollback a pending withdraw (compensation)
    ///
    /// For Funding: Unfreezes funds (frozen -= amount, available += amount)
    /// For Trading: Credit back (available += amount)
    async fn rollback(&self, req_id: RequestId) -> OpResult;

    /// Query status of an operation (optional)
    async fn query(&self, req_id: RequestId) -> OpResult;

    /// Get service name for logging
    fn name(&self) -> &str;
}
