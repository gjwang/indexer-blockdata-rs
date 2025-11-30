use fetcher::matching_engine::{MatchingEngine, SymbolManager, Side};

#[test]
fn test_symbol_manager_loading() {
    let manager = SymbolManager::load_from_db();
    assert_eq!(manager.get_id("BTC_USDT"), Some(0));
    assert_eq!(manager.get_id("ETH_USDT"), Some(1));
    assert_eq!(manager.get_symbol(0), Some(&"BTC_USDT".to_string()));
    assert_eq!(manager.get_symbol(1), Some(&"ETH_USDT".to_string()));
}

#[test]
fn test_basic_matching() {
    let mut engine = MatchingEngine::new();
    let manager = SymbolManager::load_from_db();
    
    // Register symbols
    for (symbol, &id) in &manager.symbol_to_id {
        engine.register_symbol(id, symbol.clone()).unwrap();
    }

    let btc_id = manager.get_id("BTC_USDT").unwrap();

    // 1. Add Sell Orders
    // Sell 100 @ 10 (ID 1)
    assert!(engine.add_order(btc_id, 1, Side::Sell, 100, 10).is_ok());
    // Sell 101 @ 5 (ID 2)
    assert!(engine.add_order(btc_id, 2, Side::Sell, 101, 5).is_ok());

    // Verify Book State
    let book = engine.order_books[btc_id].as_ref().unwrap();
    assert_eq!(book.asks.len(), 2);
    assert_eq!(book.bids.len(), 0);

    // 2. Add Buy Order (Partial Match)
    // Buy 100 @ 8 (ID 3). Matches 8 units of ID 1.
    assert!(engine.add_order(btc_id, 3, Side::Buy, 100, 8).is_ok());

    let book = engine.order_books[btc_id].as_ref().unwrap();
    // ID 1 should have 2 units left (10 - 8)
    let best_ask = book.asks.values().next().unwrap().front().unwrap();
    assert_eq!(best_ask.order_id, 1);
    assert_eq!(best_ask.quantity, 2);
    
    // ID 3 should be fully filled (not in bids)
    assert!(book.bids.is_empty());

    // 3. Add Buy Order (Full Match + Sweep)
    // Buy 102 @ 10 (ID 4). 
    // Should match remaining 2 of ID 1 (Price 100).
    // Should match all 5 of ID 2 (Price 101).
    // Remaining 3 units should sit on Bid at 102.
    assert!(engine.add_order(btc_id, 4, Side::Buy, 102, 10).is_ok());

    let book = engine.order_books[btc_id].as_ref().unwrap();
    assert!(book.asks.is_empty()); // All asks cleared
    assert_eq!(book.bids.len(), 1);
    
    let best_bid = book.bids.values().next().unwrap().front().unwrap();
    assert_eq!(best_bid.order_id, 4);
    assert_eq!(best_bid.quantity, 3);
    assert_eq!(best_bid.price, 102);
}

#[test]
fn test_dynamic_symbol_registration() {
    let mut engine = MatchingEngine::new();
    let mut manager = SymbolManager::load_from_db();
    
    // Initial load
    for (symbol, &id) in &manager.symbol_to_id {
        engine.register_symbol(id, symbol.clone()).unwrap();
    }

    // Add new symbol
    let new_symbol = "SOL_USDT";
    let new_id = 5;
    manager.insert(new_symbol, new_id);
    
    assert!(engine.register_symbol(new_id, new_symbol.to_string()).is_ok());
    
    // Trade on new symbol
    assert!(engine.add_order(new_id, 100, Side::Sell, 50, 100).is_ok());
    
    let book = engine.order_books[new_id].as_ref().unwrap();
    assert_eq!(book.symbol, new_symbol);
    assert_eq!(book.asks.len(), 1);
}

#[test]
fn test_gap_handling() {
    let mut engine = MatchingEngine::new();
    // Register ID 0 and 5, leaving gaps
    engine.register_symbol(0, "BTC".to_string()).unwrap();
    engine.register_symbol(5, "SOL".to_string()).unwrap();

    // Access ID 2 (Gap)
    let result = engine.add_order(2, 999, Side::Buy, 100, 1);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Symbol ID 2 is not active (gap)");
    
    // Access ID 10 (Out of bounds)
    let result = engine.add_order(10, 999, Side::Buy, 100, 1);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Invalid symbol ID: 10");
}

#[test]
fn test_duplicate_order_id() {
    let mut engine = MatchingEngine::new();
    engine.register_symbol(0, "BTC".to_string()).unwrap();

    // Add Order 1
    assert!(engine.add_order(0, 1, Side::Buy, 100, 10).is_ok());

    // Try adding Order 1 again
    let result = engine.add_order(0, 1, Side::Sell, 100, 10);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Duplicate order ID: 1");

    // Fill Order 1 fully
    // Sell 100 @ 10 (ID 2). Matches ID 1 fully.
    assert!(engine.add_order(0, 2, Side::Sell, 100, 10).is_ok());

    // Now ID 1 should be inactive (removed from set)
    // Re-using ID 1 should be allowed (or at least not blocked by active set)
    // Note: In a real system we might want global uniqueness, but current logic only checks active set.
    assert!(engine.add_order(0, 1, Side::Buy, 99, 5).is_ok());
}
