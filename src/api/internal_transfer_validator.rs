use crate::api::internal_transfer_types::error_codes;
use crate::models::internal_transfer_types::{AccountType, InternalTransferRequest};
use crate::symbol_manager::SymbolManager;
use anyhow::{bail, Result};
use rust_decimal::Decimal;

/// Validate internal transfer request
pub fn validate_transfer_request(
    req: &InternalTransferRequest,
    symbol_manager: &SymbolManager,
) -> Result<()> {
    // 1. Asset consistency
    if req.from_account.asset() != req.to_account.asset() {
        bail!("{}: from={}, to={}",
            error_codes::ASSET_MISMATCH,
            req.from_account.asset(),
            req.to_account.asset()
        );
    }

    // 2. Cannot transfer to same account
    if is_same_account(&req.from_account, &req.to_account) {
        bail!("{}: cannot transfer to same account", error_codes::SAME_ACCOUNT);
    }

    // 3. Amount must be positive
    if req.amount <= Decimal::ZERO {
        bail!("{}: amount must be positive", error_codes::INVALID_AMOUNT);
    }

    // 4. Validate precision
    let asset = req.from_account.asset();
    validate_amount_precision(&req.amount, &asset, symbol_manager)?;

    // 5. Check transfer permission
    check_transfer_permission(&req.from_account, &req.to_account)?;

    Ok(())
}

/// Check if two accounts are the same
fn is_same_account(from: &AccountType, to: &AccountType) -> bool {
    match (from, to) {
        (AccountType::Funding { .. }, AccountType::Funding { .. }) => true,
        (AccountType::Spot { user_id: uid1, .. }, AccountType::Spot { user_id: uid2, .. }) => {
            uid1 == uid2
        }
        _ => false,
    }
}

/// Validate amount precision based on asset
fn validate_amount_precision(
    amount: &Decimal,
    asset: &str,
    symbol_manager: &SymbolManager,
) -> Result<()> {
    let asset_id = symbol_manager
        .get_asset_id(asset)
        .ok_or_else(|| anyhow::anyhow!("{}: unknown asset {}", error_codes::INVALID_ASSET, asset))?;

    let decimals = symbol_manager
        .get_asset_decimal(asset_id)
        .ok_or_else(|| anyhow::anyhow!("Asset decimal not found"))?;

   // Check precision
    let scale = amount.scale();
    if scale > decimals {
        bail!(
            "{}: amount precision {} exceeds asset decimals {}",
            error_codes::INVALID_PRECISION,
            scale,
            decimals
        );
    }

    Ok(())
}

/// Check transfer permission
/// Currently: user can only transfer between their own Spot and system Funding
fn check_transfer_permission(from: &AccountType, to: &AccountType) -> Result<()> {
    match (from, to) {
        // Funding -> Spot (transfer in): allowed
        (AccountType::Funding { .. }, AccountType::Spot { .. }) => Ok(()),

        // Spot -> Funding (transfer out): allowed
        (AccountType::Spot { .. }, AccountType::Funding { .. }) => Ok(()),

        // Spot -> Spot (different users): denied
        (AccountType::Spot { user_id: uid1, .. }, AccountType::Spot { user_id: uid2, .. }) => {
            if uid1 != uid2 {
                bail!("{}: cannot transfer between different users", error_codes::PERMISSION_DENIED);
            }
            Ok(())
        }

        // All other combinations: denied
        _ => bail!("{}: invalid transfer direction", error_codes::PERMISSION_DENIED),
    }
}

/// Convert Decimal to i64 (scaled)
pub fn decimal_to_i64(amount: &Decimal, decimals: u32) -> Result<i64> {
    let scale_factor = Decimal::from(10_i64.pow(decimals));
    let scaled = amount * scale_factor;

    // Convert to string and parse to i64
    let value_str = scaled.to_string();
    let value: i64 = value_str
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|_| anyhow::anyhow!("Failed to convert amount"))?;

    if value > 0 {
        Ok(value)
    } else {
        bail!("Amount must be positive after scaling")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_symbol_manager() -> SymbolManager {
        SymbolManager::load_from_db()
    }

    #[test]
    fn test_validate_valid_transfer_in() {
        let mgr = get_symbol_manager();
        let req = InternalTransferRequest {
            from_account: AccountType::Funding {
                asset: "USDT".to_string(),
                user_id: 0,
            },
            to_account: AccountType::Spot {
                user_id: 100,
                asset: "USDT".to_string(),
            },
            amount: Decimal::new(100_000_000, 8), // 1.00 USDT
        };

        assert!(validate_transfer_request(&req, &mgr).is_ok());
    }

    #[test]
    fn test_asset_mismatch() {
        let mgr = get_symbol_manager();
        let req = InternalTransferRequest {
            from_account: AccountType::Funding {
                asset: "BTC".to_string(),
                user_id: 0,
            },
            to_account: AccountType::Spot {
                user_id: 100,
                asset: "USDT".to_string(),
            },
            amount: Decimal::new(100_000_000, 8),
        };

        assert!(validate_transfer_request(&req, &mgr).is_err());
    }

    #[test]
    fn test_same_account() {
        let mgr = get_symbol_manager();
        let req = InternalTransferRequest {
            from_account: AccountType::Spot {
                user_id: 100,
                asset: "USDT".to_string(),
            },
            to_account: AccountType::Spot {
                user_id: 100,
                asset: "USDT".to_string(),
            },
            amount: Decimal::new(100_000_000, 8),
        };

        assert!(validate_transfer_request(&req, &mgr).is_err());
    }

    #[test]
    fn test_negative_amount() {
        let mgr = get_symbol_manager();
        let req = InternalTransferRequest {
            from_account: AccountType::Funding {
                asset: "USDT".to_string(),
                user_id: 0,
            },
            to_account: AccountType::Spot {
                user_id: 100,
                asset: "USDT".to_string(),
            },
            amount: Decimal::new(-100_000_000, 8),
        };

        assert!(validate_transfer_request(&req, &mgr).is_err());
    }

    #[test]
    fn test_decimal_to_i64() {
        let amount = Decimal::new(100_000_000, 8); // 1.00
        let result = decimal_to_i64(&amount, 8).unwrap();
        assert_eq!(result, 100_000_000);
    }
}
