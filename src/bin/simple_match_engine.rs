use std::collections::{BTreeMap, VecDeque};
use rustc_hash::FxHashMap;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub order_id: u64,
    pub symbol: String,
    pub side: Side,
    pub price: u64,
    pub quantity: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub match_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub price: u64,
    pub quantity: u64,
}

use rustc_hash::FxHashSet;

pub struct OrderBook {
    pub symbol: String,
    // Bids: High to Low. We use Reverse for BTreeMap to iterate from highest price.
    pub bids: BTreeMap<std::cmp::Reverse<u64>, VecDeque<Order>>,
    // Asks: Low to High.
    pub asks: BTreeMap<u64, VecDeque<Order>>,
    pub trade_history: Vec<Trade>,
    pub order_counter: u64,
    pub match_sequence: u64,
    pub active_order_ids: FxHashSet<u64>,
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        OrderBook {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            trade_history: Vec::new(),
            order_counter: 0,
            match_sequence: 0,
            active_order_ids: FxHashSet::default(),
        }
    }

    pub fn add_order(&mut self, order_id: u64, symbol: &str, side: Side, price: u64, quantity: u64) -> Result<u64, String> {
        // Validate symbol matches this OrderBook
        if symbol != self.symbol {
            return Err(format!("Symbol mismatch: expected '{}', got '{}'", self.symbol, symbol));
        }
        
        // prevent duplicate order_id
        if self.active_order_ids.contains(&order_id) {
            return Err(format!("Duplicate order ID: {}", order_id));
        }

        self.order_counter += 1;
        let order = Order {
            order_id,
            symbol: self.symbol.clone(),
            side,
            price,
            quantity,
            timestamp: self.order_counter, // Using counter as logical timestamp
        };

        Ok(self.match_order(order))
    }

    pub fn add_order_by_symbol_id(&mut self, order_id: u64, _symbol_id: usize, side: Side, price: u64, quantity: u64) -> Result<u64, String> {
        // Validate order_id, can not duplicate insert
        if self.active_order_ids.contains(&order_id) {
            return Err(format!("Duplicate order ID: {}", order_id));
        }

        self.order_counter += 1;
        let order = Order {
            order_id,
            symbol: self.symbol.clone(),
            side,
            price,
            quantity,
            timestamp: self.order_counter,
        };

        Ok(self.match_order(order))
    }

    fn match_order(&mut self, mut order: Order) -> u64 {
        let order_id = order.order_id;
        let mut trades = Vec::new();

        match order.side {
            Side::Buy => {
                // Match against Asks (Low to High)
                while order.quantity > 0 {
                    // Check if there is a matching ask
                    let mut best_ask_price = None;
                    if let Some((price, _)) = self.asks.iter().next() {
                        if price <= &order.price {
                            best_ask_price = Some(*price);
                        }
                    }

                    if let Some(price) = best_ask_price {
                        let orders_at_price = self.asks.get_mut(&price).unwrap();
                        while let Some(mut best_ask) = orders_at_price.pop_front() {
                            let trade_quantity = u64::min(order.quantity, best_ask.quantity);
                            
                            self.match_sequence += 1;
                            trades.push(Trade {
                                match_id: self.match_sequence,
                                buy_order_id: order.order_id,
                                sell_order_id: best_ask.order_id,
                                price: best_ask.price, // Trade happens at maker's price
                                quantity: trade_quantity,
                            });

                            order.quantity -= trade_quantity;
                            best_ask.quantity -= trade_quantity;

                            if best_ask.quantity > 0 {
                                // If maker order is not fully filled, push it back to front (it has priority)
                                orders_at_price.push_front(best_ask);
                                break; // Taker is fully filled
                            } else {
                                // Maker order fully filled, remove from active set
                                self.active_order_ids.remove(&best_ask.order_id);
                            }
                            
                            if order.quantity == 0 {
                                break;
                            }
                        }
                        
                        // Clean up empty price level
                        if orders_at_price.is_empty() {
                            self.asks.remove(&price);
                        }
                    } else {
                        break; // No matching asks
                    }
                }
            }
            Side::Sell => {
                // Match against Bids (High to Low)
                while order.quantity > 0 {
                    let mut best_bid_price = None;
                    if let Some((std::cmp::Reverse(price), _)) = self.bids.iter().next() {
                        if price >= &order.price {
                            best_bid_price = Some(*price);
                        }
                    }

                    if let Some(price) = best_bid_price {
                        let orders_at_price = self.bids.get_mut(&std::cmp::Reverse(price)).unwrap();
                        while let Some(mut best_bid) = orders_at_price.pop_front() {
                            let trade_quantity = u64::min(order.quantity, best_bid.quantity);

                            self.match_sequence += 1;
                            trades.push(Trade {
                                match_id: self.match_sequence,
                                buy_order_id: best_bid.order_id,
                                sell_order_id: order.order_id,
                                price: best_bid.price,
                                quantity: trade_quantity,
                            });

                            order.quantity -= trade_quantity;
                            best_bid.quantity -= trade_quantity;

                            if best_bid.quantity > 0 {
                                orders_at_price.push_front(best_bid);
                                break;
                            } else {
                                // Maker order fully filled, remove from active set
                                self.active_order_ids.remove(&best_bid.order_id);
                            }

                            if order.quantity == 0 {
                                break;
                            }
                        }

                        if orders_at_price.is_empty() {
                            self.bids.remove(&std::cmp::Reverse(price));
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        // If order still has quantity, add to book
        if order.quantity > 0 {
            self.active_order_ids.insert(order.order_id);
            match order.side {
                Side::Buy => {
                    self.bids
                        .entry(std::cmp::Reverse(order.price))
                        .or_insert_with(VecDeque::new)
                        .push_back(order);
                }
                Side::Sell => {
                    self.asks
                        .entry(order.price)
                        .or_insert_with(VecDeque::new)
                        .push_back(order);
                }
            }
        }

        self.trade_history.extend(trades.clone());
        
        for trade in trades {
            println!("Trade Executed: {:?}", trade);
        }

        order_id
    }

    pub fn print_book(&self) {
        println!("ASKS:");
        for (price, orders) in self.asks.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price, total_qty, orders.len());
        }
        println!("------------------");
        println!("BIDS:");
        for (std::cmp::Reverse(price), orders) in self.bids.iter() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price, total_qty, orders.len());
        }
    }
}

pub struct MatchingEngine {
    pub order_books: Vec<Option<OrderBook>>,
}

impl MatchingEngine {
    pub fn new() -> Self {
        MatchingEngine {
            order_books: Vec::new(),
        }
    }

    /// Register a symbol at a specific ID
    /// Resizes the internal storage if necessary
    pub fn register_symbol(&mut self, symbol_id: usize, symbol: String) -> Result<(), String> {
        // Resize if necessary
        if symbol_id >= self.order_books.len() {
            self.order_books.resize_with(symbol_id + 1, || None);
        } else if self.order_books[symbol_id].is_some() {
            return Err(format!("Symbol ID {} is already in use", symbol_id));
        }

        self.order_books[symbol_id] = Some(OrderBook::new(symbol));
        Ok(())
    }

    /// Add order using symbol ID
    pub fn add_order(&mut self, symbol_id: usize, order_id: u64, side: Side, price: u64, quantity: u64) -> Result<u64, String> {
        let book_opt = self.order_books.get_mut(symbol_id)
            .ok_or_else(|| format!("Invalid symbol ID: {}", symbol_id))?;
            
        let book = book_opt.as_mut()
            .ok_or_else(|| format!("Symbol ID {} is not active (gap)", symbol_id))?;
        
        book.add_order_by_symbol_id(order_id, symbol_id, side, price, quantity)
    }

    pub fn print_order_book(&self, symbol_id: usize) {
        if let Some(Some(book)) = self.order_books.get(symbol_id) {
            println!("\n--- Order Book for {} (ID: {}) ---", book.symbol, symbol_id);
            book.print_book();
            println!("----------------------------------\n");
        } else {
            println!("Order book for ID {} not found", symbol_id);
        }
    }
}

/// Manages symbol-to-ID and ID-to-symbol mappings
struct SymbolManager {
    symbol_to_id: FxHashMap<String, usize>,
    id_to_symbol: FxHashMap<usize, String>, // Using HashMap for sparse ID support
}

impl SymbolManager {
    fn new() -> Self {
        SymbolManager {
            symbol_to_id: FxHashMap::default(),
            id_to_symbol: FxHashMap::default(),
        }
    }

    fn insert(&mut self, symbol: &str, id: usize) {
        self.symbol_to_id.insert(symbol.to_string(), id);
        self.id_to_symbol.insert(id, symbol.to_string());
    }

    fn get_id(&self, symbol: &str) -> Option<usize> {
        self.symbol_to_id.get(symbol).copied()
    }

    fn get_symbol(&self, id: usize) -> Option<&String> {
        self.id_to_symbol.get(&id)
    }

    /// Load initial state (simulating DB load)
    fn load_from_db() -> Self {
        let mut manager = SymbolManager::new();
        manager.insert("BTC_USDT", 0);
        manager.insert("ETH_USDT", 1);
        manager
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
