use std::path::Path;
use std::fs;
use fetcher::matching_engine_base::{MatchingEngine, SymbolManager, Side};
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

fn assert_balance(engine: &MatchingEngine, user_id: u64, asset_id: u32, expected_avail: u64, expected_frozen: u64) {
    let balances = engine.ledger.get_user_balances(user_id).expect("User not found");
    let bal = balances.iter().find(|(a, _)| *a == asset_id).expect("Asset not found").1;
    assert_eq!(bal.available, expected_avail, "User {} Asset {} Available mismatch", user_id, asset_id);
    assert_eq!(bal.frozen, expected_frozen, "User {} Asset {} Frozen mismatch", user_id, asset_id);
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

    // Verify Balances
    assert_balance(&engine, 1, 1, 9900, 0);   // User 1: BTC
    assert_balance(&engine, 1, 2, 1000, 0);   // User 1: USDT
    assert_balance(&engine, 2, 1, 9899, 0);   // User 2: BTC
    assert_balance(&engine, 2, 2, 505, 0);    // User 2: USDT
    assert_balance(&engine, 4, 1, 101, 0);    // User 4: BTC
    assert_balance(&engine, 4, 2, 98985, 10); // User 4: USDT
    println!(">>> Balance Assertions Passed!");

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

    // ==========================================
    // LOAD TEST
    // ==========================================
    println!("\n>>> STARTING LOAD TEST (1,000,000 Orders)");
    
    // Deposit funds for load test users
    // User 1000: Whale Seller (Asset 1: BTC)
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1000, asset: 1, amount: 1_000_000_000 }).unwrap();
    // User 1001: Whale Buyer (Asset 2: USDT)
    engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1001, asset: 2, amount: 50_000_000_000 }).unwrap();

    let total = 3_000;
    let start = std::time::Instant::now();
    let mut buy_orders = Vec::new();
    let mut sell_orders = Vec::new();

    // Start ID from 10000 to avoid conflict with previous manual orders
    let start_id = 500;
    let snapshot_every_n_orders = 100;

    for i in 1..=total {
        let order_id = start_id + i;
        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
        let user_id = if side == Side::Buy { 1001 } else { 1000 };
        let price = 50000;
        let quantity = 1;

        // Store order IDs for matching/cancelling
        if side == Side::Buy {
            buy_orders.push(order_id);
        } else {
            sell_orders.push(order_id);
        }

        // Place Order
        engine.add_order(btc_id, order_id, side, price, quantity, user_id).unwrap();

        // Match orders every 100 orders
        // Note: In simple_match_engine, matching happens automatically inside add_order if prices cross.
        // But here we are placing both sides at same price (50000), so they SHOULD match immediately if we alternate.
        // However, our loop alternates Buy/Sell.
        // If we place Sell 50000, it sits.
        // Then Buy 50000, it matches the Sell.
        // So simple_match_engine matches immediately! We don't need manual matching calls like me_wal.
        
        // Cancel an order every 500 orders (if any exist in book)
        if i % 500 == 0 {
            // Since they match immediately, we might not have many orders in the book unless we place non-matching ones.
            // Let's place a non-matching order to cancel.
            let cancel_id = order_id + 1_000_000; // Unique ID
            // Place a Buy low or Sell high so it doesn't match
            let (c_side, c_price) = (Side::Buy, 100); 
            engine.add_order(btc_id, cancel_id, c_side, c_price, 1, 1001).unwrap();
            
            if engine.cancel_order(btc_id, cancel_id).unwrap() {
                 // println!("    Cancelled Order {}", cancel_id);
            }
        }

        // Snapshot every 200k
        if i % snapshot_every_n_orders == 0 {
            let t = std::time::Instant::now();
            engine.trigger_cow_snapshot();
            println!("    Forked at Order {}. Main Thread Paused: {:.2?}", i, t.elapsed());
        }
    }

    let dur = start.elapsed();
    println!("\n>>> LOAD TEST DONE");
    println!("    Total Orders: {}", total);
    println!("    Total Time: {:.2?}", dur);
    println!("    Throughput: {:.0} orders/sec", total as f64 / dur.as_secs_f64());

    // Wait for children to finish
    std::thread::sleep(std::time::Duration::from_secs(3));
}
