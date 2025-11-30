use fetcher::matching_engine::{MatchingEngine, SymbolManager, Side};

fn main() {
    let mut engine = MatchingEngine::new();

    // === Simulating Database Load at Startup ===
    println!("=== Loading symbols from database (simulated) ===");
    
    // 1. Load Symbols via Manager
    let mut symbol_manager = SymbolManager::load_from_db();

    // 2. Initialize Engine with Symbols (using IDs)
    for (&symbol_id, symbol) in &symbol_manager.id_to_symbol {
        engine.register_symbol(symbol_id, symbol.clone()).unwrap();
        println!("Loaded symbol: {} -> symbol_id: {}", symbol, symbol_id);
    }
    
    println!("=== Symbol initialization complete ===\n");

    println!("\n>>> Trading BTC_USDT");
    let btc_id = symbol_manager.get_id("BTC_USDT").expect("BTC_USDT not found");
    println!("Adding Sell Order: 100 @ 10");
    engine.add_order(btc_id, 1, Side::Sell, 100, 10).unwrap();
    
    println!("Adding Sell Order: 101 @ 5");
    engine.add_order(btc_id, 2, Side::Sell, 101, 5).unwrap();

    engine.print_order_book(btc_id);

    println!("Adding Buy Order: 100 @ 8 (Should match partial 100)");
    engine.add_order(btc_id, 3, Side::Buy, 100, 8).unwrap();

    engine.print_order_book(btc_id);

    println!(">>> Trading ETH_USDT");
    let eth_id = symbol_manager.get_id("ETH_USDT").expect("ETH_USDT not found");
    println!("Adding Sell Order: 2000 @ 50");
    engine.add_order(eth_id, 101, Side::Sell, 2000, 50).unwrap();
    
    engine.print_order_book(eth_id);
    
    println!(">>> Trading BTC_USDT again (using ID directly)");
    println!("Adding Buy Order: 102 @ 10");
    engine.add_order(btc_id, 4, Side::Buy, 102, 10).unwrap();

    engine.print_order_book(btc_id);

    // === Demonstrate Dynamic Symbol Addition ===
    println!("\n>>> Dynamically adding new symbol: SOL_USDT with ID 5");
    let new_symbol = "SOL_USDT";
    let new_id = 5; 
    
    // Update Symbol Manager
    symbol_manager.insert(new_symbol, new_id);
    
    // Register in Engine
    engine.register_symbol(new_id, new_symbol.to_string()).unwrap();
    println!("Registered {} with ID: {}", new_symbol, new_id);
    
    println!("Adding Sell Order for SOL_USDT: 50 @ 100");
    engine.add_order(new_id, 201, Side::Sell, 50, 100).unwrap();
    engine.print_order_book(new_id);
}
