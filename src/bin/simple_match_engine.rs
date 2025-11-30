use std::path::Path;
use std::fs;
use fetcher::matching_engine::{MatchingEngine, SymbolManager, Side};
use fetcher::ledger::LedgerCommand;

fn print_user_balances(engine: &MatchingEngine, user_ids: &[u64]) {
    println!("--- User Balances ---");
    for &uid in user_ids {
        if let Some(balances) = engine.ledger.get_user_balances(uid) {
            print!("User {}: ", uid);
            for (asset, bal) in balances {
                print!("[Asset {}: Avail={}, Frozen={}] ", asset, bal.available, bal.frozen);
            }
            println!();
        } else {
            println!("User {}: No account", uid);
        }
    }
    println!("---------------------");
}

fn main() {
    let wal_dir = Path::new("me_wal_data");
    let snap_dir = Path::new("me_snapshots");
    
    // Clean up previous run
    if wal_dir.exists() { let _ = fs::remove_dir_all(wal_dir); }
    if snap_dir.exists() { let _ = fs::remove_dir_all(snap_dir); }

    let mut engine = MatchingEngine::new(wal_dir, snap_dir).expect("Failed to create engine");

    // === Deposit Funds ===
    println!("=== Depositing Funds ===");
    // User 1: Seller of BTC (Asset 1).
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 1, amount: 10000 }).unwrap();
    // User 2: Seller of BTC.
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 2, asset: 1, amount: 10000 }).unwrap();
    // User 3: Buyer of BTC (Asset 2 = USDT).
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 3, asset: 2, amount: 100000 }).unwrap();
    // User 4: Buyer of BTC.
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 4, asset: 2, amount: 100000 }).unwrap();
    // User 101: Seller of ETH (Asset 3).
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 101, asset: 3, amount: 10000 }).unwrap();
    // User 201: Seller of SOL (Asset 4).
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 201, asset: 4, amount: 10000 }).unwrap();
    println!("Funds deposited.\n");
    
    print_user_balances(&engine, &[1, 2, 3, 4, 101, 201]);

    // === Simulating Database Load at Startup ===
    println!("=== Loading symbols from database (simulated) ===");
    
    // 1. Load Symbols via Manager
    let symbol_manager = SymbolManager::load_from_db();

    // 2. Initialize Engine with Symbols (using IDs)
    for (&symbol_id, symbol) in &symbol_manager.id_to_symbol {
        let (base, quote) = match symbol.as_str() {
            "BTC_USDT" => (1, 2),
            "ETH_USDT" => (3, 2),
            _ => (100, 2),
        };
        engine.register_symbol(symbol_id, symbol.clone(), base, quote).unwrap();
        println!("Loaded symbol: {} -> symbol_id: {} (Base: {}, Quote: {})", symbol, symbol_id, base, quote);
    }
    
    println!("=== Symbol initialization complete ===\n");

    println!("\n>>> Trading BTC_USDT");
    let btc_id = symbol_manager.get_id("BTC_USDT").expect("BTC_USDT not found");
    println!("Adding Sell Order: 100 @ 10 (User 1)");
    engine.add_order(btc_id, 1, Side::Sell, 10, 100, 1).unwrap();
    
    println!("Adding Sell Order: 101 @ 5 (User 2)");
    engine.add_order(btc_id, 2, Side::Sell, 5, 101, 2).unwrap();

    engine.print_order_book(btc_id);
    print_user_balances(&engine, &[1, 2]);

    println!("Adding Buy Order: 100 @ 8 (User 3) (Should match partial 100)");
    engine.add_order(btc_id, 3, Side::Buy, 8, 100, 3).unwrap();

    engine.print_order_book(btc_id);
    print_user_balances(&engine, &[1, 2, 3]);

    println!(">>> Trading ETH_USDT");
    let eth_id = symbol_manager.get_id("ETH_USDT").expect("ETH_USDT not found");
    println!("Adding Sell Order: 2000 @ 50 (User 101)");
    engine.add_order(eth_id, 101, Side::Sell, 50, 2000, 101).unwrap();
    
    engine.print_order_book(eth_id);
    
    println!(">>> Trading BTC_USDT again (using ID directly)");
    println!("Adding Buy Order: 102 @ 10 (User 4)");
    engine.add_order(btc_id, 4, Side::Buy, 10, 102, 4).unwrap();

    engine.print_order_book(btc_id);
    print_user_balances(&engine, &[1, 2, 4]);

    // === Demonstrate Dynamic Symbol Addition ===
    println!("\n>>> Dynamically adding new symbol: SOL_USDT with ID 5");
    let new_symbol = "SOL_USDT";
    let new_id = 5; 
    
    // Register in Engine
    engine.register_symbol(new_id, new_symbol.to_string(), 4, 2).unwrap();
    println!("Registered {} with ID: {}", new_symbol, new_id);
    
    println!("Adding Sell Order for SOL_USDT: 50 @ 100 (User 201)");
    engine.add_order(new_id, 201, Side::Sell, 50, 100, 201).unwrap();
    engine.print_order_book(new_id);
    print_user_balances(&engine, &[201]);
}
