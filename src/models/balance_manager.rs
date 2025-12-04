use std::sync::Arc;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

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


/// BalanceManager handles the conversion between client-facing decimal amounts (e.g., "1.5 BTC")
/// and internal integer representations (e.g., 150_000_000 satoshis).
///
/// # Key Features
/// - **Precision Enforcement**: Ensures client inputs do not exceed `display_decimals` (for assets) or `price_decimal` (for prices).
/// - **Safe Arithmetic**: Uses checked operations to prevent overflows.
/// - **Rounding**: Rounds output values to `display_decimals` or `price_decimal` for consistent client display.
///
/// # Usage
/// - Use `to_internal_amount` / `to_client_amount` for asset quantities (balances).
/// - Use `to_internal_price` / `to_client_price` for trading prices.
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

    /// Converts an internal integer amount to a client-facing Decimal.
    ///
    /// # Returns
    /// * `Option<Decimal>` - The decimal amount rounded to `display_decimals`.
    ///
    /// # Notes
    /// * This method rounds towards zero (truncates) to match `display_decimals`.
    /// * Returns `None` if asset is unknown or calculation overflows.
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

    /// Converts a client-facing Decimal amount to an internal integer representation.
    ///
    /// # Arguments
    /// * `asset_name` - The name of the asset (e.g., "BTC").
    /// * `amount` - The amount in decimal format.
    ///
    /// # Returns
    /// * `Result<(u32, u64), String>` - The asset ID and the raw internal amount.
    ///
    /// # Errors
    /// * If the asset is unknown.
    /// * If the amount exceeds the allowed `display_decimals` precision.
    /// * If the conversion results in an overflow (e.g., decimals too large).
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

        let result = (amount * multiplier).round();

        let raw_amount = result
            .to_u64()
            .ok_or_else(|| format!("Amount overflow or negative: {}", result))?;

        Ok((asset_id, raw_amount))
    }

    /// Converts a client-facing Decimal price to an internal integer representation.
    ///
    /// # Arguments
    /// * `symbol` - The trading symbol (e.g., "BTC_USDT").
    /// * `price` - The price in decimal format.
    ///
    /// # Returns
    /// * `Result<u64, String>` - The raw internal price.
    ///
    /// # Errors
    /// * If the symbol is unknown.
    /// * If the price exceeds the allowed `price_display_decimal` precision.
    /// * If the conversion results in an overflow.
    pub fn to_internal_price(&self, symbol: &str, price: Decimal) -> Result<u64, String> {
        let symbol_info = self
            .symbol_manager
            .get_symbol_info(symbol)
            .ok_or_else(|| format!("Unknown symbol: {}", symbol))?;

        let decimals = symbol_info.price_decimal;
        let display_decimals = symbol_info.price_display_decimal;

        // Validate precision
        if price.normalize().scale() > display_decimals {
            return Err(format!("Price {} exceeds max precision {}", price, display_decimals));
        }

        let multiplier = Decimal::from(
            10_u64.checked_pow(decimals).ok_or("Price decimals overflow")?,
        );

        let result = (price * multiplier).round();

        result
            .to_u64()
            .ok_or_else(|| format!("Price overflow or negative: {}", result))
    }

    /// Converts an internal integer price to a client-facing Decimal.
    ///
    /// # Returns
    /// * `Option<Decimal>` - The exact decimal price without rounding.
    ///
    /// # Notes
    /// * Returns `None` if symbol is unknown or calculation overflows.
    /// * Does NOT round - returns exact value as stored internally.
    pub fn to_client_price(&self, symbol: &str, price: u64) -> Option<Decimal> {
        let symbol_info = self.symbol_manager.get_symbol_info(symbol)?;
        let decimals = symbol_info.price_decimal;

        let divisor = Decimal::from(10_u64.checked_pow(decimals)?);

        Some(Decimal::from(price) / divisor)
    }

    /// Converts an internal integer price to a client-facing Decimal using symbol ID.
    ///
    /// # Returns
    /// * `Option<Decimal>` - The exact decimal price without rounding.
    ///
    /// # Notes
    /// * Returns `None` if symbol is unknown or calculation overflows.
    /// * Does NOT round - returns exact value as stored internally.
    pub fn to_client_price_by_id(&self, symbol_id: u32, price: u64) -> Option<Decimal> {
        let symbol_info = self.symbol_manager.get_symbol_info_by_id(symbol_id)?;
        let decimals = symbol_info.price_decimal;

        let divisor = Decimal::from(10_u64.checked_pow(decimals)?);

        Some(Decimal::from(price) / divisor)
    }
}
