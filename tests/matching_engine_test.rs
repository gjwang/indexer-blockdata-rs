use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fetcher::ledger::LedgerCommand;
use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::{OrderType, Side};
use fetcher::symbol_manager::SymbolManager;

fn setup_engine(test_name: &str) -> (MatchingEngine, PathBuf, PathBuf) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base_dir = PathBuf::from("test_data");
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir).unwrap();
    }
    let wal_dir = base_dir.join(format!("test_wal_{}_{}", test_name, ts));
    let snap_dir = base_dir.join(format!("test_snap_{}_{}", test_name, ts));

    if wal_dir.exists() {
        fs::remove_dir_all(&wal_dir).unwrap();
    }
    if snap_dir.exists() {
        fs::remove_dir_all(&snap_dir).unwrap();
    }

    let mut engine = MatchingEngine::new(&wal_dir, &snap_dir, false).unwrap();

    // Deposit funds for generic users
    // User 1: Seller (Base Asset 1)
    engine
        .ledger
        .apply(&LedgerCommand::Deposit { user_id: 1, asset_id: 1, amount: 1_000_000, balance_after: 0, version: 0 })
        .unwrap();
    // User 2: Seller (Base Asset 1)
    engine
        .ledger
        .apply(&LedgerCommand::Deposit { user_id: 2, asset_id: 1, amount: 1_000_000, balance_after: 0, version: 0 })
        .unwrap();
    // User 3: Buyer (Quote Asset 2)
    engine
        .ledger
        .apply(&LedgerCommand::Deposit { user_id: 3, asset_id: 2, amount: 10_000_000, balance_after: 0, version: 0 })
        .unwrap();
    // User 4: Buyer (Quote Asset 2)
    engine
        .ledger
        .apply(&LedgerCommand::Deposit { user_id: 4, asset_id: 2, amount: 10_000_000, balance_after: 0, version: 0 })
        .unwrap();

    (engine, wal_dir, snap_dir)
}

fn teardown(wal_dir: PathBuf, snap_dir: PathBuf) {
    if wal_dir.exists() {
        fs::remove_dir_all(wal_dir).unwrap();
    }
    if snap_dir.exists() {
        fs::remove_dir_all(snap_dir).unwrap();
    }
}

#[test]
fn test_symbol_manager_loading() {
    let manager = SymbolManager::load_from_db();
    assert_eq!(manager.get_symbol_id("BTC_USDT"), Some(0));
    assert_eq!(manager.get_symbol_id("ETH_USDT"), Some(1));
    assert_eq!(manager.get_symbol(0), Some(&"BTC_USDT".to_string()));
    assert_eq!(manager.get_symbol(1), Some(&"ETH_USDT".to_string()));
}

#[test]
fn test_basic_matching() {
    let (mut engine, wal, snap) = setup_engine("basic");
    let manager = SymbolManager::load_from_db();

    // Register symbols
    for (symbol, &id) in &manager.symbol_to_id {
        engine.register_symbol(id, symbol.clone(), 1, 2).unwrap();
    }

    let btc_id = manager.get_symbol_id("BTC_USDT").unwrap();

    // 1. Add Sell Orders
    // Sell 100 @ 10 (ID 1, User 1) -> Actually Price 100, Qty 10 based on original code
    assert!(engine.add_order(btc_id, 1, Side::Sell, OrderType::Limit, 100, 10, 1, 0).is_ok());
    // Sell 101 @ 5 (ID 2, User 2) -> Actually Price 101, Qty 5
    assert!(engine.add_order(btc_id, 2, Side::Sell, OrderType::Limit, 101, 5, 2, 0).is_ok());

    // Verify Book State
    let book = engine.order_books[btc_id as usize].as_ref().unwrap();
    assert_eq!(book.asks.len(), 2);
    assert_eq!(book.bids.len(), 0);

    // 2. Add Buy Order (Partial Match)
    // Buy 100 @ 8 (ID 3, User 3). Matches 8 units of ID 1 (Price 100).
    assert!(engine.add_order(btc_id, 3, Side::Buy, OrderType::Limit, 100, 8, 3, 0).is_ok());

    let book = engine.order_books[btc_id as usize].as_ref().unwrap();
    // ID 1 (Price 100) matched 8. Remaining 2.
    let best_ask = book.asks.values().next().unwrap().front().unwrap();
    assert_eq!(best_ask.order_id, 1);
    assert_eq!(best_ask.quantity, 2);

    // 3. Add Buy Order (Full Match + Sweep)
    // Buy 102 @ 10 (ID 4, User 4).
    // Matches remaining 2 of ID 1 (Price 100).
    // Matches 5 of ID 2 (Price 101).
    // Remaining 3 units sit on Bid at 102.
    assert!(engine.add_order(btc_id, 4, Side::Buy, OrderType::Limit, 102, 10, 4, 0).is_ok());

    let book = engine.order_books[btc_id as usize].as_ref().unwrap();
    assert!(book.asks.is_empty()); // All asks cleared
    assert_eq!(book.bids.len(), 1);

    let best_bid = book.bids.values().next().unwrap().front().unwrap();
    assert_eq!(best_bid.order_id, 4);
    assert_eq!(best_bid.quantity, 3);
    assert_eq!(best_bid.price, 102);

    drop(engine);
    teardown(wal, snap);
}

#[test]
fn test_dynamic_symbol_registration() {
    let (mut engine, wal, snap) = setup_engine("dynamic");
    let mut manager = SymbolManager::load_from_db();

    // Initial load
    for (symbol, &id) in &manager.symbol_to_id {
        engine.register_symbol(id, symbol.clone(), 1, 2).unwrap();
    }

    // Add new symbol
    let new_symbol = "SOL_USDT";
    let new_id = 5;
    manager.add_asset(4, 8, 4, "SOL"); // SOL
    manager.insert(new_symbol, new_id, 4, 2);

    assert!(engine.register_symbol(new_id, new_symbol.to_string(), 4, 2).is_ok());

    // Trade on new symbol
    // Need deposit for User 1 Asset 4 (SOL)
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset_id: 4, amount: 1000, balance_after: 0, version: 0 }).unwrap();

    assert!(engine.add_order(new_id, 100, Side::Sell, OrderType::Limit, 50, 100, 1, 0).is_ok());

    let book = engine.order_books[new_id as usize].as_ref().unwrap();
    assert_eq!(book.symbol_id, new_id as u32);
    assert_eq!(book.asks.len(), 1);

    drop(engine);
    teardown(wal, snap);
}

#[test]
fn test_gap_handling() {
    let (mut engine, wal, snap) = setup_engine("gap");
    // Register ID 0 and 5, leaving gaps
    engine.register_symbol(0, "BTC".to_string(), 1, 2).unwrap();
    engine.register_symbol(5, "SOL".to_string(), 4, 2).unwrap();

    // Access ID 2 (Gap)
    let result = engine.add_order(2, 999, Side::Buy, OrderType::Limit, 1, 100, 3, 0);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), fetcher::models::OrderError::InvalidSymbol { .. }));

    // Access ID 10 (Out of bounds)
    let result = engine.add_order(10, 999, Side::Buy, OrderType::Limit, 1, 100, 3, 0);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), fetcher::models::OrderError::InvalidSymbol { .. }));

    drop(engine);
    teardown(wal, snap);
}

#[test]
fn test_duplicate_order_id() {
    let (mut engine, wal, snap) = setup_engine("duplicate");
    engine.register_symbol(0, "BTC".to_string(), 1, 2).unwrap();

    // Add Order 1 (Buy 100 @ 10)
    engine.add_order(0, 1, Side::Buy, OrderType::Limit, 10, 100, 3, 0).unwrap();

    // Add Order 2 (Sell 50 @ 10) -> Match 50
    engine.add_order(0, 2, Side::Sell, OrderType::Limit, 10, 50, 1, 0).unwrap();

    // Add Order 3 (Sell 40 @ 10) -> Match 40, Rem 10 (Order 1 still active)
    engine.add_order(0, 3, Side::Sell, OrderType::Limit, 10, 40, 2, 0).unwrap();

    // Try adding Order 1 again (should fail as it's still active/partially filled)
    let result = engine.add_order(0, 1, Side::Sell, OrderType::Limit, 10, 100, 1, 0);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), fetcher::models::OrderError::DuplicateOrderId { .. }));

    // Cancel Order 1 to make it inactive
    assert!(engine.cancel_order(0, 1).is_ok());

    // Now ID 1 should be inactive (removed from set)
    // Re-using ID 1 should be allowed
    assert!(engine.add_order(0, 1, Side::Buy, OrderType::Limit, 5, 99, 3, 0).is_ok());

    drop(engine);
    teardown(wal, snap);
}

#[test]
fn test_ledger_integration() {
    let (mut engine, wal, snap) = setup_engine("ledger");
    let manager = SymbolManager::load_from_db();
    let btc_id = manager.get_symbol_id("BTC_USDT").unwrap();
    engine.register_symbol(btc_id, "BTC_USDT".to_string(), 1, 2).unwrap();

    // User 1: Sell 100 BTC @ 10 USDT
    // Price 10, Qty 100.
    // Lock: 100 BTC (Asset 1).
    assert!(engine.add_order(btc_id, 1, Side::Sell, OrderType::Limit, 10, 100, 1, 0).is_ok());

    // Check User 1 Balance
    // Initial: 1,000,000.
    // Frozen: 100.
    // Avail: 999,900.
    let bals = engine.ledger.get_user_balances(1).unwrap();
    let btc = bals.iter().find(|(a, _)| *a == 1).unwrap().1;
    assert_eq!(btc.avail, 999_900);
    assert_eq!(btc.frozen, 100);

    // User 3: Buy 50 BTC @ 10 USDT
    // Price 10, Qty 50.
    // Lock: 50 * 10 = 500 USDT (Asset 2).
    // Match: 50 units.
    // Trade: Price 10, Qty 50.
    assert!(engine.add_order(btc_id, 2, Side::Buy, OrderType::Limit, 10, 50, 3, 0).is_ok());

    // Check User 1 (Seller)
    // Sold 50.
    // Frozen: 100 - 50 = 50.
    // Avail: 999,900.
    // Gained USDT: 50 * 10 = 500.
    let bals1 = engine.ledger.get_user_balances(1).unwrap();
    let btc1 = bals1.iter().find(|(a, _)| *a == 1).unwrap().1;
    assert_eq!(btc1.frozen, 50);
    assert_eq!(btc1.avail, 999_900);

    let usdt1 = bals1.iter().find(|(a, _)| *a == 2);
    // User 1 didn't have USDT initially? setup_engine gives generic deposits.
    // User 1 only got Asset 1.
    // So now they have Asset 2.
    assert!(usdt1.is_some());
    let usdt1 = usdt1.unwrap().1;
    assert_eq!(usdt1.avail, 500);

    // Check User 3 (Buyer)
    // Initial Asset 2: 10,000,000.
    // Locked: 500.
    // Spent: 500.
    // Gained BTC: 50.
    let bals3 = engine.ledger.get_user_balances(3).unwrap();
    let usdt3 = bals3.iter().find(|(a, _)| *a == 2).unwrap().1;
    assert_eq!(usdt3.frozen, 0); // Fully matched
    assert_eq!(usdt3.avail, 10_000_000 - 500);

    let btc3 = bals3.iter().find(|(a, _)| *a == 1);
    assert!(btc3.is_some());
    assert_eq!(btc3.unwrap().1.avail, 50);

    drop(engine);
    teardown(wal, snap);
}
