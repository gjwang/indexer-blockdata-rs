use std::sync::Arc;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromStr;
use fetcher::models::balance_manager::{BalanceManager, ClientBalance, InternalBalance};
use fetcher::symbol_manager::SymbolManager;
use fetcher::user_account::Balance;

#[test]
fn test_balance_manager_conversion_precision() {
    let mut sm = SymbolManager::new();
    // Asset 1: BTC, decimals=8, display=3
    sm.add_asset(1, 8, 3, "BTC");
    // Asset 2: USDT, decimals=6, display=2
    sm.add_asset(2, 6, 2, "USDT");

    let bm = BalanceManager::new(Arc::new(sm));

    // Test Case 1: Valid conversion (BTC)
    // 1.234 BTC -> 123400000 (internal)
    let amount = Decimal::from_str("1.234").unwrap();
    let (asset_id, raw) = bm.to_internal_amount("BTC", amount).expect("Conversion failed");
    assert_eq!(asset_id, 1);
    assert_eq!(raw, 123_400_000);

    // Test Case 2: Precision limit (BTC)
    // 1.2345 BTC -> Error (max display decimals 3)
    let amount = Decimal::from_str("1.2345").unwrap();
    let result = bm.to_internal_amount("BTC", amount);
    assert!(result.is_err(), "Should fail due to precision limit");
    assert_eq!(result.unwrap_err(), "Amount 1.2345 exceeds max precision 3");

    // Test Case 3: Valid conversion (USDT)
    // 10.50 USDT -> 10500000 (internal, decimals 6)
    let amount = Decimal::from_str("10.50").unwrap();
    let (asset_id, raw) = bm.to_internal_amount("USDT", amount).expect("Conversion failed");
    assert_eq!(asset_id, 2);
    assert_eq!(raw, 10_500_000);

    // Test Case 4: Zero value
    let amount = Decimal::from(0);
    let (_, raw) = bm.to_internal_amount("BTC", amount).unwrap();
    assert_eq!(raw, 0);

    // Test Case 5: Round trip
    // Internal 123400000 -> Client 1.234
    let client_amount = bm.to_client_amount(1, 123_400_000).unwrap();
    assert_eq!(client_amount.to_string(), "1.234");
}

#[test]
fn test_balance_manager_overflow() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 8, "BTC");
    let bm = BalanceManager::new(Arc::new(sm));

    // Max u64 is approx 1.84e19
    // With 8 decimals, max client amount is approx 1.84e11

    // Test valid large number
    let amount = Decimal::from_str("100000000000.00000000").unwrap(); // 1e11
    let (_, raw) = bm.to_internal_amount("BTC", amount).unwrap();
    assert_eq!(raw, 10_000_000_000_000_000_000);

    // Test overflow
    // 2e11 -> 2e19 > u64::MAX
    let amount = Decimal::from_str("200000000000.00000000").unwrap();
    let result = bm.to_internal_amount("BTC", amount);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Amount overflow");
}

#[test]
fn test_balance_manager_struct_conversion() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    let bm = BalanceManager::new(Arc::new(sm));

    // Client -> Internal
    let client_bal = ClientBalance {
        asset: "BTC".to_string(),
        avail: Decimal::from_str("1.5").unwrap(),
        frozen: Decimal::from_str("0.1").unwrap(),
    };

    let internal = bm.to_internal_balance_struct(client_bal).expect("Struct conversion failed");
    assert_eq!(internal.asset_id, 1);
    assert_eq!(internal.balance.avail, 150_000_000);
    assert_eq!(internal.balance.frozen, 10_000_000);

    // Internal -> Client
    let client_back = bm.to_client_balance_struct(internal).expect("Back conversion failed");
    assert_eq!(client_back.asset, "BTC");
    assert_eq!(client_back.avail, Decimal::from_str("1.5").unwrap());
    assert_eq!(client_back.frozen, Decimal::from_str("0.1").unwrap());
}

#[test]
fn test_balance_manager_decimals_overflow() {
    let mut sm = SymbolManager::new();
    // Decimals 20 -> 10^20 overflows u64
    sm.add_asset(1, 20, 20, "HUGE");
    let bm = BalanceManager::new(Arc::new(sm));

    let amount = Decimal::from(1);
    let result = bm.to_internal_amount("HUGE", amount);
    // This should currently panic or return error if I fix it.
    // If it panics, the test runner catches it?
    // I want to assert it returns Err, not panic.
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Decimals too large, overflow");
}

#[test]
fn test_unknown_asset() {
    let sm = SymbolManager::new();
    let bm = BalanceManager::new(Arc::new(sm));

    let amount = Decimal::from(1);
    let result = bm.to_internal_amount("UNKNOWN", amount);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Unknown asset: UNKNOWN");
}
