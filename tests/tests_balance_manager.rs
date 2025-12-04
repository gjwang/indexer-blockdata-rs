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
    assert!(result.unwrap_err().contains("Amount overflow"));
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
fn test_balance_manager_price_conversion() {
    let mut sm = SymbolManager::new();
    // Asset 1: BTC, decimals=8, display=3
    sm.add_asset(1, 8, 3, "BTC");
    // Asset 2: USDT, decimals=6, display=2
    sm.add_asset(2, 6, 2, "USDT");
    // Symbol: BTC_USDT, price_decimal=2
    sm.insert("BTC_USDT", 1, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test to_internal_price
    // 50000.50 -> 5000050
    let price = Decimal::from_str("50000.50").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Price conversion failed");
    assert_eq!(internal, 5000050);

    // Test precision check
    // 50000.501 -> Error (precision 2)
    let price_invalid = Decimal::from_str("50000.501").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price_invalid);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("exceeds max precision"));

    // Test to_client_price
    // 5000050 -> 50000.50
    let client_price = bm.to_client_price("BTC_USDT", 5000050).unwrap();
    assert_eq!(client_price, Decimal::from_str("50000.50").unwrap());
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

#[test]
fn test_price_conversion_strict() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    // Symbol: BTC_USDT, price_decimal=2, price_display_decimal=2
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test Case 1: Valid price conversion (exact precision)
    // 50000.50 -> 5000050
    let price = Decimal::from_str("50000.50").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Valid price should convert");
    assert_eq!(internal, 5000050);

    // Test Case 2: Valid price conversion (less than max precision)
    // 50000.5 -> 5000050
    let price = Decimal::from_str("50000.5").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Valid price should convert");
    assert_eq!(internal, 5000050);

    // Test Case 3: Valid price conversion (integer)
    // 50000 -> 5000000
    let price = Decimal::from(50000);
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Valid price should convert");
    assert_eq!(internal, 5000000);

    // Test Case 4: Zero price
    let price = Decimal::from(0);
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Zero price should convert");
    assert_eq!(internal, 0);

    // Test Case 5: Very small price (within precision)
    // 0.01 -> 1
    let price = Decimal::from_str("0.01").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Small price should convert");
    assert_eq!(internal, 1);

    // Test Case 6: Maximum precision (2 decimals)
    // 99999.99 -> 9999999
    let price = Decimal::from_str("99999.99").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Max precision should convert");
    assert_eq!(internal, 9999999);
}

#[test]
fn test_price_conversion_precision_validation() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    // price_decimal=2, price_display_decimal=2
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test Case 1: Exceeds display precision (3 decimals when max is 2)
    // 50000.501 -> Error
    let price = Decimal::from_str("50000.501").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Price 50000.501 exceeds max precision 2");

    // Test Case 2: Far exceeds display precision
    // 50000.12345 -> Error
    let price = Decimal::from_str("50000.12345").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("exceeds max precision"));

    // Test Case 3: Tiny amount with too much precision
    // 0.001 -> Error (3 decimals when max is 2)
    let price = Decimal::from_str("0.001").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err());
}

#[test]
fn test_price_conversion_different_precisions() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");

    // Symbol 1: price_decimal=4, price_display_decimal=2
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 4, 2);
    // Symbol 2: price_decimal=8, price_display_decimal=4
    sm.insert_symbol("ETH_USDT", 2, 3, 2, 8, 4);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test BTC_USDT (price_decimal=4, display=2)
    // 1234.56 -> 12345600
    let price = Decimal::from_str("1234.56").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Should convert");
    assert_eq!(internal, 12345600);

    // 1234.567 -> Error (3 decimals when display max is 2)
    let price = Decimal::from_str("1234.567").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err());

    // Test ETH_USDT (price_decimal=8, display=4)
    // 1234.5678 -> 123456780000
    let price = Decimal::from_str("1234.5678").unwrap();
    let internal = bm.to_internal_price("ETH_USDT", price).expect("Should convert");
    assert_eq!(internal, 123456780000);

    // 1234.56789 -> Error (5 decimals when display max is 4)
    let price = Decimal::from_str("1234.56789").unwrap();
    let result = bm.to_internal_price("ETH_USDT", price);
    assert!(result.is_err());
}

#[test]
fn test_price_conversion_round_trip() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test Case 1: Round trip with exact precision
    let original = Decimal::from_str("50000.50").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", original).unwrap();
    let back = bm.to_client_price("BTC_USDT", internal).unwrap();
    assert_eq!(back, original);

    // Test Case 2: Round trip with less precision
    let original = Decimal::from_str("50000.5").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", original).unwrap();
    let back = bm.to_client_price("BTC_USDT", internal).unwrap();
    assert_eq!(back, Decimal::from_str("50000.50").unwrap()); // Normalized

    // Test Case 3: Round trip with integer
    let original = Decimal::from(50000);
    let internal = bm.to_internal_price("BTC_USDT", original).unwrap();
    let back = bm.to_client_price("BTC_USDT", internal).unwrap();
    assert_eq!(back, Decimal::from_str("50000.00").unwrap());

    // Test Case 4: Round trip with zero
    let original = Decimal::from(0);
    let internal = bm.to_internal_price("BTC_USDT", original).unwrap();
    let back = bm.to_client_price("BTC_USDT", internal).unwrap();
    assert_eq!(back, Decimal::from(0));
}

#[test]
fn test_price_conversion_overflow() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    // price_decimal=2
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // u64::MAX is ~18,446,744,073,709,551,615
    // With price_decimal=2, max representable price is ~184,467,440,737,095,516.15

    // Test valid large price
    let price = Decimal::from_str("100000000000000.00").unwrap();
    let internal = bm.to_internal_price("BTC_USDT", price).expect("Large price should convert");
    assert_eq!(internal, 10000000000000000);

    // Test overflow
    // 200000000000000000.00 -> 20000000000000000000 > u64::MAX
    let price = Decimal::from_str("200000000000000000.00").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Price overflow"));
}

#[test]
fn test_price_conversion_negative() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Negative price should fail
    let price = Decimal::from_str("-100.50").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("negative"));
}

#[test]
fn test_price_conversion_by_id() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test to_client_price_by_id
    let internal = 5000050_u64;
    let client = bm.to_client_price_by_id(1, internal).unwrap();
    assert_eq!(client, Decimal::from_str("50000.50").unwrap());

    // Test unknown symbol ID
    let result = bm.to_client_price_by_id(999, internal);
    assert!(result.is_none());
}

#[test]
fn test_price_conversion_unknown_symbol() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");

    let bm = BalanceManager::new(Arc::new(sm));

    // Test to_internal_price with unknown symbol
    let price = Decimal::from_str("50000.50").unwrap();
    let result = bm.to_internal_price("UNKNOWN_SYMBOL", price);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Unknown symbol: UNKNOWN_SYMBOL");

    // Test to_client_price with unknown symbol
    let result = bm.to_client_price("UNKNOWN_SYMBOL", 5000050);
    assert!(result.is_none());
}
