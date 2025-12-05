// Symbol mapping utilities for settlement service

use anyhow::{bail, Result};

// Asset IDs (should match matching engine)
pub const BTC: u32 = 1;
pub const USDT: u32 = 2;
pub const ETH: u32 = 3;

/// Get symbol name from base and quote asset IDs
///
/// # Arguments
/// * `base_asset` - Base asset ID (e.g., BTC = 1)
/// * `quote_asset` - Quote asset ID (e.g., USDT = 2)
///
/// # Returns
/// * Symbol name in format "base_quote" (e.g., "btc_usdt")
pub fn get_symbol_from_assets(base_asset: u32, quote_asset: u32) -> Result<String> {
    let base_name = match base_asset {
        BTC => "btc",
        ETH => "eth",
        _ => bail!("Unknown base asset: {}", base_asset),
    };

    let quote_name = match quote_asset {
        USDT => "usdt",
        BTC => "btc",
        ETH => "eth",
        _ => bail!("Unknown quote asset: {}", quote_asset),
    };

    Ok(format!("{}_{}", base_name, quote_name))
}

/// Get asset name from asset ID
pub fn get_asset_name(asset_id: u32) -> Result<&'static str> {
    match asset_id {
        BTC => Ok("BTC"),
        USDT => Ok("USDT"),
        ETH => Ok("ETH"),
        _ => bail!("Unknown asset: {}", asset_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_symbol_from_assets() {
        assert_eq!(get_symbol_from_assets(BTC, USDT).unwrap(), "btc_usdt");
        assert_eq!(get_symbol_from_assets(ETH, USDT).unwrap(), "eth_usdt");
        assert_eq!(get_symbol_from_assets(BTC, ETH).unwrap(), "btc_eth");
    }

    #[test]
    fn test_get_asset_name() {
        assert_eq!(get_asset_name(BTC).unwrap(), "BTC");
        assert_eq!(get_asset_name(USDT).unwrap(), "USDT");
        assert_eq!(get_asset_name(ETH).unwrap(), "ETH");
    }
}
