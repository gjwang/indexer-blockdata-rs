use serde::{Deserialize, Serialize};

pub type AssetId = u32;
pub type UserId = u64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[repr(C)]
pub struct Balance {
    pub avail: u64,
    pub frozen: u64,
    pub version: u64,
}

impl Balance {
    pub fn deposit(&mut self, amount: u64) -> Result<(), &'static str> {
        self.avail = self.avail.checked_add(amount).ok_or("Balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn withdraw(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.avail < amount {
            return Err("Insufficient funds");
        }
        self.avail = self.avail.checked_sub(amount).ok_or("Balance underflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn frozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.avail < amount {
            return Err("Insufficient funds");
        }
        self.avail = self.avail.checked_sub(amount).ok_or("Balance underflow")?;
        self.frozen = self.frozen.checked_add(amount).ok_or("Frozen balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn unfrozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.frozen < amount {
            return Err("Insufficient frozen funds");
        }
        self.frozen = self.frozen.checked_sub(amount).ok_or("Frozen balance underflow")?;
        self.avail = self.avail.checked_add(amount).ok_or("Balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn spend_frozen(&mut self, amount: u64) -> Result<(), &'static str> {
        if self.frozen < amount {
            return Err("Insufficient frozen funds");
        }
        self.frozen = self.frozen.checked_sub(amount).ok_or("Frozen balance underflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    pub user_id: UserId,
    pub assets: Vec<(AssetId, Balance)>,
}

impl UserAccount {
    pub fn new(user_id: UserId) -> Self {
        Self {
            user_id,
            assets: Vec::with_capacity(8),
        }
    }
    #[inline(always)]
    pub fn get_balance_mut(&mut self, asset: AssetId) -> &mut Balance {
        if let Some(index) = self.assets.iter().position(|(a, _)| *a == asset) {
            return &mut self.assets[index].1;
        }
        self.assets.push((
            asset,
            Balance {
                avail: 0,
                frozen: 0,
                version: 0,
            },
        ));
        &mut self.assets.last_mut().unwrap().1
    }

    #[inline(always)]
    pub fn get_balance(&self, asset: AssetId) -> Option<&Balance> {
        self.assets
            .iter()
            .find(|(a, _)| *a == asset)
            .map(|(_, b)| b)
    }

    pub fn check_buyer_balance(
        &self,
        quote_asset: AssetId,
        spend_quote: u64,
        refund_quote: u64,
    ) -> Result<(), &'static str> {
        let quote_bal = self.get_balance(quote_asset).ok_or("Quote asset not found")?;
        
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
        base_asset: AssetId,
        spend_base: u64,
        refund_base: u64,
    ) -> Result<(), &'static str> {
        let base_bal = self.get_balance(base_asset).ok_or("Base asset not found")?;

        let required = spend_base + refund_base;
        if base_bal.frozen < required {
            return Err("Insufficient frozen base funds");
        }
        Ok(())
    }
    pub fn settle_as_buyer(
        &mut self,
        quote_asset: AssetId,
        base_asset: AssetId,
        spend_quote: u64,
        gain_base: u64,
        refund_quote: u64,
    ) -> Result<(), &'static str> {
        // Debit Quote (Frozen)
        let quote_bal = self.get_balance_mut(quote_asset);
        quote_bal.spend_frozen(spend_quote)?;

        // Credit Base (Available)
        let base_bal = self.get_balance_mut(base_asset);
        base_bal.deposit(gain_base)?;

        // Refund Quote (Frozen -> Available)
        if refund_quote > 0 {
            let quote_bal = self.get_balance_mut(quote_asset);
            quote_bal.unfrozen(refund_quote)?;
        }
        Ok(())
    }

    pub fn settle_as_seller(
        &mut self,
        base_asset: AssetId,
        quote_asset: AssetId,
        spend_base: u64,
        gain_quote: u64,
        refund_base: u64,
    ) -> Result<(), &'static str> {
        // Debit Base (Frozen)
        let base_bal = self.get_balance_mut(base_asset);
        base_bal.spend_frozen(spend_base)?;

        // Credit Quote (Available)
        let quote_bal = self.get_balance_mut(quote_asset);
        quote_bal.deposit(gain_quote)?;

        // Refund Base (Frozen -> Available)
        if refund_base > 0 {
            let base_bal = self.get_balance_mut(base_asset);
            base_bal.unfrozen(refund_base)?;
        }
        Ok(())
    }
}
