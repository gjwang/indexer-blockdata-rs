use std::sync::Arc;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromStr;
use fetcher::models::balance_manager::BalanceManager;
use fetcher::symbol_manager::SymbolManager;

/// Tests for INVALID price inputs - precision violations
#[test]
fn test_invalid_price_precision_boundary() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    // price_decimal=2, price_display_decimal=2
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Exactly 1 decimal over limit (3 decimals when max is 2)
    let invalid_prices = vec![
        "0.001",      // 3 decimals
        "1.001",      // 3 decimals
        "100.001",    // 3 decimals
        "50000.501",  // 3 decimals
        "99999.991",  // 3 decimals
    ];

    for price_str in invalid_prices {
        let price = Decimal::from_str(price_str).unwrap();
        let result = bm.to_internal_price("BTC_USDT", price);
        assert!(result.is_err(), "Price {} should be rejected (3 decimals)", price_str);
        assert!(result.unwrap_err().contains("exceeds max precision"));
    }
}

/// Tests for INVALID price inputs - far exceeding precision
#[test]
fn test_invalid_price_excessive_precision() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Way over precision limit
    let invalid_prices = vec![
        "0.0001",           // 4 decimals
        "0.00001",          // 5 decimals
        "1.123456789",      // 9 decimals
        "50000.123456",     // 6 decimals
        "99999.999999999",  // 9 decimals
    ];

    for price_str in invalid_prices {
        let price = Decimal::from_str(price_str).unwrap();
        let result = bm.to_internal_price("BTC_USDT", price);
        assert!(result.is_err(), "Price {} should be rejected", price_str);
    }
}

/// Tests for INVALID price inputs - negative values
#[test]
fn test_invalid_price_negative_values() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Test negative values with valid precision (so precision check passes, negative check fails)
    let invalid_prices = vec![
        "-0.01",
        "-1.00",
        "-100.50",
        "-50000.00",
    ];

    for price_str in invalid_prices {
        let price = Decimal::from_str(price_str).unwrap();
        let result = bm.to_internal_price("BTC_USDT", price);
        assert!(result.is_err(), "Negative price {} should be rejected", price_str);
        assert!(result.unwrap_err().contains("negative"),
                "Error should mention 'negative' for price {}", price_str);
    }

    // Note: negative with excess precision will fail precision check first
    let price = Decimal::from_str("-0.001").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err(), "Should be rejected (precision or negative)");
}

/// Tests for INVALID price inputs - overflow scenarios
#[test]
fn test_invalid_price_overflow_boundary() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // u64::MAX = 18,446,744,073,709,551,615
    // With price_decimal=2, max = 184,467,440,737,095,516.15

    let overflow_prices = vec![
        "184467440737095517.00",  // Just over max
        "200000000000000000.00",  // Way over
        "999999999999999999.99",  // Extreme
    ];

    for price_str in overflow_prices {
        let price = Decimal::from_str(price_str).unwrap();
        let result = bm.to_internal_price("BTC_USDT", price);
        assert!(result.is_err(), "Overflow price {} should be rejected", price_str);
        assert!(result.unwrap_err().contains("overflow"),
                "Error should mention 'overflow' for price {}", price_str);
    }
}

/// Tests for INVALID amount inputs - precision violations
#[test]
fn test_invalid_amount_precision_boundary() {
    let mut sm = SymbolManager::new();
    // BTC: decimals=8, display=3
    sm.add_asset(1, 8, 3, "BTC");

    let bm = BalanceManager::new(Arc::new(sm));

    // Exactly 1 decimal over limit (4 decimals when max is 3)
    let invalid_amounts = vec![
        "0.0001",     // 4 decimals
        "1.0001",     // 4 decimals
        "100.1234",   // 4 decimals
        "0.9999",     // 4 decimals
    ];

    for amount_str in invalid_amounts {
        let amount = Decimal::from_str(amount_str).unwrap();
        let result = bm.to_internal_amount("BTC", amount);
        assert!(result.is_err(), "Amount {} should be rejected (4 decimals)", amount_str);
        assert_eq!(result.unwrap_err(), format!("Amount {} exceeds max precision 3", amount));
    }
}

/// Tests for INVALID amount inputs - excessive precision
#[test]
fn test_invalid_amount_excessive_precision() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");

    let bm = BalanceManager::new(Arc::new(sm));

    let invalid_amounts = vec![
        "0.00001",          // 5 decimals
        "1.123456",         // 6 decimals
        "100.12345678",     // 8 decimals
        "0.999999999",      // 9 decimals
    ];

    for amount_str in invalid_amounts {
        let amount = Decimal::from_str(amount_str).unwrap();
        let result = bm.to_internal_amount("BTC", amount);
        assert!(result.is_err(), "Amount {} should be rejected", amount_str);
    }
}

/// Tests for INVALID amount inputs - negative values
#[test]
fn test_invalid_amount_negative_values() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");

    let bm = BalanceManager::new(Arc::new(sm));

    // Test negative values with valid precision
    let invalid_amounts = vec![
        "-0.001",
        "-1.0",
        "-100.5",
    ];

    for amount_str in invalid_amounts {
        let amount = Decimal::from_str(amount_str).unwrap();
        let result = bm.to_internal_amount("BTC", amount);
        assert!(result.is_err(), "Negative amount {} should be rejected", amount_str);
        assert!(result.unwrap_err().contains("negative"),
                "Error should mention 'negative' for amount {}", amount_str);
    }

    // Note: negative with excess precision will fail precision check first
    let amount = Decimal::from_str("-0.0001").unwrap();
    let result = bm.to_internal_amount("BTC", amount);
    assert!(result.is_err(), "Should be rejected (precision or negative)");
}

/// Tests for INVALID amount inputs - overflow scenarios
#[test]
fn test_invalid_amount_overflow_boundary() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 8, "BTC");

    let bm = BalanceManager::new(Arc::new(sm));

    // With decimals=8, max = 184,467,440,737.09551615

    let overflow_amounts = vec![
        "184467440738.0",        // Just over
        "200000000000.0",        // Way over
        "999999999999.99999999", // Extreme
    ];

    for amount_str in overflow_amounts {
        let amount = Decimal::from_str(amount_str).unwrap();
        let result = bm.to_internal_amount("BTC", amount);
        assert!(result.is_err(), "Overflow amount {} should be rejected", amount_str);
        assert!(result.unwrap_err().contains("overflow"),
                "Error should mention 'overflow' for amount {}", amount_str);
    }
}

/// Tests for INVALID inputs - unknown assets/symbols
#[test]
fn test_invalid_unknown_asset() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");

    let bm = BalanceManager::new(Arc::new(sm));

    let unknown_assets = vec!["ETH", "USDT", "DOGE", "XRP", ""];

    for asset in unknown_assets {
        let amount = Decimal::from(100);
        let result = bm.to_internal_amount(asset, amount);
        assert!(result.is_err(), "Unknown asset '{}' should be rejected", asset);
        assert_eq!(result.unwrap_err(), format!("Unknown asset: {}", asset));
    }
}

/// Tests for INVALID inputs - unknown symbols
#[test]
fn test_invalid_unknown_symbol() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    let unknown_symbols = vec!["ETH_USDT", "BTC_ETH", "DOGE_USDT", "XRP_BTC", ""];

    for symbol in unknown_symbols {
        let price = Decimal::from(100);
        let result = bm.to_internal_price(symbol, price);
        assert!(result.is_err(), "Unknown symbol '{}' should be rejected", symbol);
        assert_eq!(result.unwrap_err(), format!("Unknown symbol: {}", symbol));
    }
}

/// Tests for INVALID inputs - edge case: very small non-zero values
#[test]
fn test_invalid_tiny_values_with_precision() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 2, "USDT");  // display_decimals=2

    let bm = BalanceManager::new(Arc::new(sm));

    // These are valid in absolute terms but exceed display precision
    let invalid_amounts = vec![
        "0.001",      // 3 decimals when max is 2
        "0.0001",     // 4 decimals
        "0.00000001", // 8 decimals
    ];

    for amount_str in invalid_amounts {
        let amount = Decimal::from_str(amount_str).unwrap();
        let result = bm.to_internal_amount("USDT", amount);
        assert!(result.is_err(), "Tiny amount {} with excess precision should be rejected", amount_str);
    }
}

/// Tests for INVALID inputs - combined violations
#[test]
fn test_invalid_combined_violations() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Negative + excess precision
    let price = Decimal::from_str("-100.123").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err(), "Negative price with excess precision should be rejected");

    // Very large + excess precision (might overflow or fail precision check first)
    let price = Decimal::from_str("999999999999999.123").unwrap();
    let result = bm.to_internal_price("BTC_USDT", price);
    assert!(result.is_err(), "Large price with excess precision should be rejected");
}

/// Tests for INVALID inputs - precision at different decimal configurations
#[test]
fn test_invalid_precision_various_configs() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");

    // Different precision configs
    sm.insert_symbol("SYM1", 1, 1, 2, 2, 0);  // display=0 (integers only)
    sm.insert_symbol("SYM2", 2, 1, 2, 4, 1);  // display=1
    sm.insert_symbol("SYM3", 3, 1, 2, 8, 4);  // display=4

    let bm = BalanceManager::new(Arc::new(sm));

    // SYM1: display=0, should reject any decimals
    let result = bm.to_internal_price("SYM1", Decimal::from_str("100.1").unwrap());
    assert!(result.is_err(), "SYM1 should reject 1 decimal");

    let result = bm.to_internal_price("SYM1", Decimal::from_str("100.01").unwrap());
    assert!(result.is_err(), "SYM1 should reject 2 decimals");

    // SYM2: display=1, should reject 2+ decimals
    let result = bm.to_internal_price("SYM2", Decimal::from_str("100.12").unwrap());
    assert!(result.is_err(), "SYM2 should reject 2 decimals");

    // SYM3: display=4, should reject 5+ decimals
    let result = bm.to_internal_price("SYM3", Decimal::from_str("100.12345").unwrap());
    assert!(result.is_err(), "SYM3 should reject 5 decimals");
}

/// Tests for INVALID inputs - boundary between valid and invalid
#[test]
fn test_invalid_precision_exact_boundary() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC");
    sm.add_asset(2, 6, 2, "USDT");
    sm.insert_symbol("BTC_USDT", 1, 1, 2, 2, 2);

    let bm = BalanceManager::new(Arc::new(sm));

    // Valid: exactly at limit (2 decimals)
    let valid = Decimal::from_str("100.99").unwrap();
    assert!(bm.to_internal_price("BTC_USDT", valid).is_ok());

    // Invalid: one digit over (3 decimals)
    let invalid = Decimal::from_str("100.991").unwrap();
    assert!(bm.to_internal_price("BTC_USDT", invalid).is_err());

    // Valid: 1 decimal
    let valid = Decimal::from_str("100.9").unwrap();
    assert!(bm.to_internal_price("BTC_USDT", valid).is_ok());

    // Valid: 0 decimals
    let valid = Decimal::from(100);
    assert!(bm.to_internal_price("BTC_USDT", valid).is_ok());
}
