use std::sync::Arc;

use rust_decimal::Decimal;

use crate::symbol_manager::SymbolManager;
use crate::user_account::Balance;

#[derive(Debug, serde::Serialize)]
pub struct ClientBalance {
    pub asset: String,
    pub avail: Decimal,
    pub frozen: Decimal,
}

#[derive(Debug)]
pub struct InternalBalance {
    pub asset_id: u32,
    pub balance: Balance,
}


pub struct BalanceManager {
    symbol_manager: Arc<SymbolManager>,
}

impl BalanceManager {
    pub fn new(symbol_manager: Arc<SymbolManager>) -> Self {
        Self { symbol_manager }
    }

    pub fn to_client_balance_struct(&self, internal: InternalBalance) -> Option<ClientBalance> {
        self.to_client_balance(internal.asset_id, internal.balance.avail, internal.balance.frozen)
    }

    pub fn to_internal_balance_struct(
        &self,
        client: ClientBalance,
    ) -> Result<InternalBalance, String> {
        let (asset_id, avail) = self.to_internal_amount(&client.asset, client.avail)?;
        let (_, frozen) = self.to_internal_amount(&client.asset, client.frozen)?;

        Ok(InternalBalance { asset_id, balance: Balance { avail, frozen, version: 0 } })
    }

    pub fn to_client_balance(
        &self,
        asset_id: u32,
        avail: u64,
        frozen: u64,
    ) -> Option<ClientBalance> {
        let asset_name = self.symbol_manager.get_asset_name(asset_id)?;

        Some(ClientBalance {
            asset: asset_name,
            avail: self.to_client_amount(asset_id, avail)?,
            frozen: self.to_client_amount(asset_id, frozen)?,
        })
    }

    pub fn to_client_amount(&self, asset_id: u32, amount: u64) -> Option<Decimal> {
        let decimals = self.symbol_manager.get_asset_decimal(asset_id)?;
        let display_decimals =
            self.symbol_manager.get_asset_display_decimals(asset_id).unwrap_or(decimals);
        let divisor = Decimal::from(10_u64.checked_pow(decimals)?);

        Some(
            (Decimal::from(amount) / divisor)
                .round_dp_with_strategy(display_decimals, rust_decimal::RoundingStrategy::ToZero),
        )
    }

    pub fn to_internal_amount(
        &self,
        asset_name: &str,
        amount: Decimal,
    ) -> Result<(u32, u64), String> {
        let asset_id = self
            .symbol_manager
            .get_asset_id(asset_name)
            .ok_or_else(|| format!("Unknown asset: {}", asset_name))?;

        // decimals: Internal storage scale (e.g., 8 for BTC)
        let decimals = self
            .symbol_manager
            .get_asset_decimal(asset_id)
            .ok_or_else(|| format!("Decimal not found for asset: {}", asset_name))?;

        // display_decimals: Max allowed decimal places for input (e.g., 3 for BTC)
        let display_decimals =
            self.symbol_manager.get_asset_display_decimals(asset_id).unwrap_or(decimals);

        // Validate input precision
        // Example: if display_decimals is 3, input 1.234 is valid, 1.2345 is invalid.
        if amount.normalize().scale() > display_decimals {
            return Err(format!("Amount {} exceeds max precision {}", amount, display_decimals));
        }
        let multiplier = Decimal::from(
            10_u64.checked_pow(decimals).ok_or("Decimals too large, overflow")?,
        );

        let raw_amount = (amount * multiplier)
            .round()
            .to_string()
            .parse::<u64>()
            .map_err(|_| "Amount overflow".to_string())?;

        Ok((asset_id, raw_amount))
    }
}
