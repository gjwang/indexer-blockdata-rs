use serde::{Deserialize, Serialize};

pub type AssetId = u32;
pub type UserId = u64;

/// Balance represents a user's balance for a single asset.
///
/// # Invariants (enforced by private fields):
/// 1. avail and frozen are NEVER negative (u64)
/// 2. No overflow: all operations use checked arithmetic
/// 3. version increments on every mutation (audit trail)
/// 4. Can only be modified through validated methods
///
/// ## Why Private Fields?
/// Making fields private ENFORCES that all mutations go through
/// validation methods. This prevents:
/// - Direct field access bypassing checks
/// - Forgetting to increment version
/// - Silent overflows
/// - Breaking invariants
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[repr(C)]
pub struct Balance {
    avail: u64,      // PRIVATE - use deposit()/withdraw()
    frozen: u64,     // PRIVATE - use frozen()/unfrozen()
    version: u64,    // PRIVATE - auto-incremented
}

impl Balance {
    /// Read-only access to available balance
    #[inline(always)]
    pub fn get_avail(&self) -> u64 {
        self.avail
    }

    /// Read-only access to frozen balance
    #[inline(always)]
    pub fn get_frozen(&self) -> u64 {
        self.frozen
    }

    /// Read-only access to version
    #[inline(always)]
    pub fn get_version(&self) -> u64 {
        self.version
    }

    /// Deposit funds to available balance
    /// Checks for overflow, increments version
    pub fn deposit(&mut self, amount: u64) -> Result<(), &'static str> {
        self.avail = self.avail.checked_add(amount).ok_or("Balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    /// Withdraw funds from available balance
    /// Checks for sufficient funds and underflow, increments version
    pub fn withdraw(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.avail < amount {
            return Err("Insufficient funds");
        }
        self.avail = self.avail.checked_sub(amount).ok_or("Balance underflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    /// Lock funds (move from available to frozen)
    /// Checks for sufficient funds, increments version
    pub fn frozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.avail < amount {
            return Err("Insufficient funds");
        }
        self.avail = self.avail.checked_sub(amount).ok_or("Balance underflow")?;
        self.frozen = self.frozen.checked_add(amount).ok_or("Frozen balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    /// Unlock funds (move from frozen to available)
    /// Checks for sufficient frozen funds, increments version
    pub fn unfrozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.frozen < amount {
            return Err("Insufficient frozen funds");
        }
        self.frozen = self.frozen.checked_sub(amount).ok_or("Frozen balance underflow")?;
        self.avail = self.avail.checked_add(amount).ok_or("Balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    /// Spend frozen funds (remove from frozen, don't add to available)
    /// Used for trade settlement
    /// Checks for sufficient frozen funds, increments version
    pub fn spend_frozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.frozen < amount {
            return Err("Insufficient frozen funds");
        }
        self.frozen = self.frozen.checked_sub(amount).ok_or("Frozen balance underflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }
}

/// UserAccount represents a user's account with balances across multiple assets.
///
/// # Invariants (enforced by private fields):
/// 1. user_id is immutable after creation
/// 2. assets can only be accessed through get_balance methods
/// 3. All mutations go through validated operations
///
/// ## Why Private Fields?
/// - Prevents direct manipulation of assets Vec
/// - Enforces get_balance_mut creates missing assets atomically
/// - No way to bypass balance validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    user_id: UserId,                      // PRIVATE - use user_id()
    assets: Vec<(AssetId, Balance)>,      // PRIVATE - use get_balance methods
}

impl UserAccount {
    pub fn new(user_id: UserId) -> Self {
        Self { user_id, assets: Vec::with_capacity(8) }
    }

    /// Read-only access to user ID
    #[inline(always)]
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    /// Get mutable reference to balance for an asset
    /// Creates the asset entry if it doesn't exist
    #[inline(always)]
    pub fn get_balance_mut(&mut self, asset_id: AssetId) -> &mut Balance {
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset_id) {
            return &mut self.assets[index].1;
        }
        self.assets.push((asset_id, Balance::default()));
        &mut self.assets.last_mut().unwrap().1
    }

    /// Get immutable reference to balance for an asset
    /// Returns None if asset doesn't exist
    #[inline(always)]
    pub fn get_balance(&self, asset_id: AssetId) -> Option<&Balance> {
        self.assets.iter().find(|(a, _)| *a == asset_id).map(|(_, b)| b)
    }

    /// Get read-only iterator over all (AssetId, Balance) pairs
    /// Used by ledger for snapshotting
    #[inline(always)]
    pub fn assets(&self) -> &[(AssetId, Balance)] {
        &self.assets
    }

    pub fn check_buyer_balance(
        &self,
        quote_asset_id: AssetId,
        spend_quote: u64,
        refund_quote: u64,
    ) -> Result<(), &'static str> {
        let quote_bal = self.get_balance(quote_asset_id).ok_or("Quote asset not found")?;

        // Check if we have enough frozen funds for both the spend and the refund
        // Usually these come from the same frozen bucket.
        let required = spend_quote + refund_quote;
        if quote_bal.frozen < required {
            return Err("Insufficient frozen quote funds");
        }
        Ok(())
    }

    pub fn check_seller_balance(
        &self,
        base_asset_id: AssetId,
        spend_base: u64,
        refund_base: u64,
    ) -> Result<(), &'static str> {
        let base_bal = self.get_balance(base_asset_id).ok_or("Base asset not found")?;

        let required = spend_base + refund_base;
        if base_bal.frozen < required {
            return Err("Insufficient frozen base funds");
        }
        Ok(())
    }
    pub fn settle_as_buyer(
        &mut self,
        quote_asset_id: AssetId,
        base_asset_id: AssetId,
        spend_quote: u64,
        gain_base: u64,
        refund_quote: u64,
    ) -> Result<(), &'static str> {
        // Debit Quote (Frozen)
        let quote_bal = self.get_balance_mut(quote_asset_id);
        quote_bal.spend_frozen(spend_quote)?;

        // Credit Base (Available)
        let base_bal = self.get_balance_mut(base_asset_id);
        base_bal.deposit(gain_base)?;

        // Refund Quote (Frozen -> Available)
        if refund_quote > 0 {
            let quote_bal = self.get_balance_mut(quote_asset_id);
            quote_bal.unfrozen(refund_quote)?;
        }
        Ok(())
    }

    pub fn settle_as_seller(
        &mut self,
        base_asset_id: AssetId,
        quote_asset_id: AssetId,
        spend_base: u64,
        gain_quote: u64,
        refund_base: u64,
    ) -> Result<(), &'static str> {
        // Debit Base (Frozen)
        let base_bal = self.get_balance_mut(base_asset_id);
        base_bal.spend_frozen(spend_base)?;

        // Credit Quote (Available)
        let quote_bal = self.get_balance_mut(quote_asset_id);
        quote_bal.deposit(gain_quote)?;

        // Refund Base (Frozen -> Available)
        if refund_base > 0 {
            let base_bal = self.get_balance_mut(base_asset_id);
            base_bal.unfrozen(refund_base)?;
        }
        Ok(())
    }
}
