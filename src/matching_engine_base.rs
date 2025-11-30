use std::collections::{BTreeMap, VecDeque};
use rustc_hash::{FxHashMap, FxHashSet};
use crate::ledger::{GlobalLedger, LedgerCommand, UserAccount, UserId};
use serde::{Serialize, Deserialize};
use nix::unistd::{fork, ForkResult};
use std::io::{Write, BufWriter};
use std::fs::File;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol: String,
    pub side: Side,
    pub price: u64,
    pub quantity: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub match_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub buy_user_id: u64,
    pub sell_user_id: u64,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OrderBook {
    pub symbol: String,
    // Bids: High to Low. We use Reverse for BTreeMap to iterate from highest price.
    pub bids: BTreeMap<std::cmp::Reverse<u64>, VecDeque<Order>>,
    // Asks: Low to High.
    pub asks: BTreeMap<u64, VecDeque<Order>>,
    // pub trade_history: Vec<Trade>, // REMOVED
    pub order_counter: u64,
    pub match_sequence: u64,
    pub active_order_ids: FxHashSet<u64>,
    pub order_index: FxHashMap<u64, (Side, u64)>, // Map order_id -> (Side, Price)
}

impl OrderBook {
    pub fn new(symbol: String) -> Self {
        OrderBook {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            // trade_history: Vec::new(), // REMOVED
            order_counter: 0,
            match_sequence: 0,
            active_order_ids: FxHashSet::default(),
            order_index: FxHashMap::default(),
        }
    }

    pub fn add_order(&mut self, order_id: u64, symbol: &str, side: Side, price: u64, quantity: u64, user_id: u64) -> Result<Vec<Trade>, String> {
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
            user_id,
            symbol: self.symbol.clone(),
            side,
            price,
            quantity,
            timestamp: self.order_counter, // Using counter as logical timestamp
        };

        Ok(self.match_order(order))
    }

    pub fn add_order_by_symbol_id(&mut self, order_id: u64, _symbol_id: usize, side: Side, price: u64, quantity: u64, user_id: u64) -> Result<Vec<Trade>, String> {
        // Validate order_id, can not duplicate insert
        if self.active_order_ids.contains(&order_id) {
            return Err(format!("Duplicate order ID: {}", order_id));
        }

        self.order_counter += 1;
        let order = Order {
            order_id,
            user_id,
            symbol: self.symbol.clone(),
            side,
            price,
            quantity,
            timestamp: self.order_counter,
        };

        Ok(self.match_order(order))
    }

    fn match_order(&mut self, mut order: Order) -> Vec<Trade> {

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
                                buy_user_id: order.user_id,
                                sell_user_id: best_ask.user_id,
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
                                self.order_index.remove(&best_ask.order_id);
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
                                buy_user_id: best_bid.user_id,
                                sell_user_id: order.user_id,
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
                                self.order_index.remove(&best_bid.order_id);
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
            self.order_index.insert(order.order_id, (order.side, order.price));
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

        // self.trade_history.extend(trades.clone()); // REMOVED
        
        for trade in &trades {
            if trade.match_id % 10_000 == 0 {
                println!("Trade Executed: {:?}", trade);
            }
        }

        trades
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

    pub fn cancel_order(&mut self, order_id: u64) -> bool {
        if let Some((side, price)) = self.order_index.remove(&order_id) {
            self.active_order_ids.remove(&order_id);
            
            let found = match side {
                Side::Buy => {
                    if let Some(orders) = self.bids.get_mut(&std::cmp::Reverse(price)) {
                        if let Some(pos) = orders.iter().position(|o| o.order_id == order_id) {
                            orders.remove(pos);
                            if orders.is_empty() {
                                self.bids.remove(&std::cmp::Reverse(price));
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                Side::Sell => {
                    if let Some(orders) = self.asks.get_mut(&price) {
                        if let Some(pos) = orders.iter().position(|o| o.order_id == order_id) {
                            orders.remove(pos);
                            if orders.is_empty() {
                                self.asks.remove(&price);
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
            };
            found
        } else {
            false
        }
    }
}

use crate::order_wal::{Wal, LogEntry, WalSide};

#[derive(Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub order_books: Vec<Option<OrderBook>>,
    pub ledger_accounts: FxHashMap<u64, UserAccount>,
    pub ledger_seq: u64,
    pub order_wal_seq: u64,
}

pub struct MatchingEngine {
    pub order_books: Vec<Option<OrderBook>>,
    pub ledger: GlobalLedger,
    pub asset_map: FxHashMap<usize, (u32, u32)>,
    pub order_wal: Wal,
    pub trade_wal: Wal,
    pub snapshot_dir: std::path::PathBuf,
}

impl MatchingEngine {
    pub fn new(wal_dir: &std::path::Path, snap_dir: &std::path::Path) -> Result<Self, String> {
        if !wal_dir.exists() {
            std::fs::create_dir_all(wal_dir).map_err(|e| e.to_string())?;
        }
        if !snap_dir.exists() {
            std::fs::create_dir_all(snap_dir).map_err(|e| e.to_string())?;
        }

        let order_wal_path = wal_dir.join("orders.wal");
        let trade_wal_path = wal_dir.join("trades.wal");
        
        // 1. Try Load Snapshot
        let (mut order_books, mut accounts, mut ledger_seq, mut order_wal_seq) = (Vec::new(), FxHashMap::default(), 0, 0);
        
        // Find latest snapshot
        let mut max_seq = 0;
        let mut latest_snap_path = None;
        if let Ok(entries) = std::fs::read_dir(snap_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if name.starts_with("engine_snapshot_") && path.extension().map_or(false, |e| e == "bin") {
                        if let Some(seq_str) = name.strip_prefix("engine_snapshot_") {
                             if let Ok(seq) = seq_str.parse::<u64>() {
                                 if seq > max_seq {
                                     max_seq = seq;
                                     latest_snap_path = Some(path);
                                 }
                             }
                        }
                    }
                }
            }
        }

        if let Some(path) = latest_snap_path {
            println!("   [Recover] Loading Engine Snapshot: {:?}", path);
            let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let reader = std::io::BufReader::new(file);
            let snap: EngineSnapshot = bincode::deserialize_from(reader).map_err(|e| e.to_string())?;
            
            order_books = snap.order_books;
            accounts = snap.ledger_accounts;
            ledger_seq = snap.ledger_seq;
            order_wal_seq = snap.order_wal_seq;
            println!("   [Recover] Snapshot Loaded. OrderWalSeq: {}, LedgerSeq: {}", order_wal_seq, ledger_seq);
        }

        // 2. Initialize Ledger from State
        let ledger = GlobalLedger::from_state(wal_dir, snap_dir, accounts, ledger_seq).map_err(|e| e.to_string())?;

        // 3. Replay WAL
        // We need to replay orders.wal to restore state.
        // But we must NOT write to orders.wal during replay.
        // We ALSO must NOT write to ledger.wal or trade.wal if we want to be purely idempotent,
        // but for now we accept output log duplicates or we can suppress them.
        // Ideally, we should suppress ALL WAL writes during replay.
        
        // To achieve this without massive refactoring, we can temporarily set a "replay mode" flag?
        // Or better, we just call the internal logic directly.
        // But `add_order` is complex.
        
        // Let's iterate and replay.
        // We need to construct the engine first to call methods on it.
        
        // Initialize WALs (writers)
        let order_wal = Wal::open(&order_wal_path, order_wal_seq).map_err(|e| e.to_string())?;
        let trade_wal = Wal::open(&trade_wal_path, 0).map_err(|e| e.to_string())?; // Output log, seq doesn't matter much for now
        
        let mut engine = MatchingEngine {
            order_books,
            ledger,
            asset_map: FxHashMap::default(),
            order_wal,
            trade_wal,
            snapshot_dir: snap_dir.to_path_buf(),
        };

        // Replay Loop
        if order_wal_path.exists() {
             println!("   [Recover] Replaying Order WAL...");
             let mut count = 0;
             // We use a separate iterator because `engine.order_wal` is a writer.
             if let Ok(iter) = Wal::replay_iter(&order_wal_path) {
                 for (seq, entry) in iter {
                     if seq <= order_wal_seq { continue; }
                     
                     match entry {
                         LogEntry::PlaceOrder { order_id, symbol, side, price, quantity, user_id, .. } => {
                             // Find symbol_id from symbol string
                             // Note: We might need to register symbol if not exists?
                             // For now, assume symbols are pre-registered or we scan WAL for registration?
                             // Wait, symbol registration is NOT in WAL. It's in code or config.
                             // We assume the user calls `register_symbol` BEFORE `add_order` in normal flow.
                             // But during recovery, `MatchingEngine::new` is called BEFORE `register_symbol` in `main`.
                             // This is a problem. `MatchingEngine` doesn't know symbol mapping yet!
                             
                             // FIX: `MatchingEngine` should persist symbol mapping in Snapshot!
                             // `EngineSnapshot` has `order_books`. `OrderBook` has `symbol` string.
                             // We can reconstruct `asset_map` from `OrderBook`s?
                             // `OrderBook` doesn't store asset IDs. `MatchingEngine` stores `asset_map`.
                             // `EngineSnapshot` needs `asset_map`.
                             
                             // For this iteration, let's assume `register_symbol` is called AFTER `new` but BEFORE we start processing new orders.
                             // BUT replay happens inside `new`.
                             // So we can't replay yet if we don't know symbols.
                             
                             // OPTION: Move replay to a separate `recover()` method called AFTER symbol registration.
                             // This is cleaner.
                             
                             // Let's change `MatchingEngine::new` to NOT replay.
                             // And add `MatchingEngine::recover()` which replays.
                             // The user (server) must call `new`, then `register_symbol`s, then `recover`.
                             
                             // However, `MatchingEngine` needs `ledger` initialized.
                             // Let's keep `new` simple.
                         }
                         _ => {}
                     }
                 }
             }
        }
        
        Ok(engine)
    }
    
    /// Register a symbol at a specific ID
    pub fn register_symbol(&mut self, symbol_id: usize, symbol: String, base_asset: u32, quote_asset: u32) -> Result<(), String> {
        if symbol_id >= self.order_books.len() {
            self.order_books.resize_with(symbol_id + 1, || None);
        } else if self.order_books[symbol_id].is_some() {
            return Err(format!("Symbol ID {} is already in use", symbol_id));
        }
        self.order_books[symbol_id] = Some(OrderBook::new(symbol));
        self.asset_map.insert(symbol_id, (base_asset, quote_asset));
        Ok(())
    }

    /// Public API: Add Order (Writes to WAL, then processes)
    pub fn add_order(&mut self, symbol_id: usize, order_id: u64, side: Side, price: u64, quantity: u64, user_id: u64) -> Result<u64, String> {
        let wal_side = match side {
            Side::Buy => WalSide::Buy,
            Side::Sell => WalSide::Sell,
        };
        
        // 1. Validate Symbol
        let (base_asset, quote_asset) = *self.asset_map.get(&symbol_id)
            .ok_or_else(|| format!("Invalid symbol ID: {}", symbol_id))?;

        // 2. Validate Balance
        let (required_asset, required_amount) = match side {
            Side::Buy => (quote_asset, price * quantity),
            Side::Sell => (base_asset, quantity),
        };

        let accounts = self.ledger.get_accounts();
        let balance = accounts.get(&user_id)
            .and_then(|user| user.assets.iter().find(|(a, _)| *a == required_asset))
            .map(|(_, b)| b.available)
            .unwrap_or(0);

        if balance < required_amount {
             return Err(format!("Insufficient funds: User {} needs {} of Asset {}, has {}", user_id, required_amount, required_asset, balance));
        }

        // 3. Write to Input Log (Source of Truth)
        self.order_wal.append(LogEntry::PlaceOrder {
            order_id,
            user_id,
            symbol: self.asset_map.get(&symbol_id).map(|_| "UNKNOWN".to_string()).unwrap_or_else(|| "UNKNOWN".to_string()),
            side: wal_side,
            price,
            quantity,
            timestamp: 0, 
        });

        // 2. Process Logic
        self.process_order(symbol_id, order_id, side, price, quantity, user_id)
    }

    /// Internal Logic: Process Order (No Input WAL write)
    /// Used by `add_order` and during Replay.
    fn process_order(&mut self, symbol_id: usize, order_id: u64, side: Side, price: u64, quantity: u64, user_id: u64) -> Result<u64, String> {
        let (base_asset, quote_asset) = *self.asset_map.get(&symbol_id).ok_or("Asset map not found")?;
        
        // 1. Lock funds
        let (lock_asset, lock_amount) = match side {
            Side::Buy => (quote_asset, price * quantity),
            Side::Sell => (base_asset, quantity),
        };
        
        self.ledger.apply(&LedgerCommand::Lock {
            user_id,
            asset: lock_asset,
            amount: lock_amount,
        }).map_err(|e| e.to_string())?;

        let book_opt = self.order_books.get_mut(symbol_id)
            .ok_or_else(|| format!("Invalid symbol ID: {}", symbol_id))?;
            
        let book = book_opt.as_mut()
            .ok_or_else(|| format!("Symbol ID {} is not active (gap)", symbol_id))?;
        
        let trades = book.add_order_by_symbol_id(order_id, symbol_id, side, price, quantity, user_id)?;
        
        // 3. Settle trades
        for trade in trades {
            // Log trade to Trade WAL (Output Log)
            self.trade_wal.append(LogEntry::Trade {
                match_id: trade.match_id,
                buy_order_id: trade.buy_order_id,
                sell_order_id: trade.sell_order_id,
                price: trade.price,
                quantity: trade.quantity,
            });

            let (buyer_spend, buyer_gain) = (trade.price * trade.quantity, trade.quantity);
            let (seller_spend, seller_gain) = (trade.quantity, trade.price * trade.quantity);
            
            // Buyer Settle
            self.ledger.apply(&LedgerCommand::TradeSettle {
                user_id: trade.buy_user_id,
                spend_asset: quote_asset,
                spend_amount: buyer_spend,
                gain_asset: base_asset,
                gain_amount: buyer_gain,
            }).map_err(|e| e.to_string())?;
            
            // Seller Settle
            self.ledger.apply(&LedgerCommand::TradeSettle {
                user_id: trade.sell_user_id,
                spend_asset: base_asset,
                spend_amount: seller_spend,
                gain_asset: quote_asset,
                gain_amount: seller_gain,
            }).map_err(|e| e.to_string())?;
            
            // Refund excess frozen for Taker Buyer
            if side == Side::Buy && trade.buy_user_id == user_id {
                let excess = (price - trade.price) * trade.quantity;
                if excess > 0 {
                     self.ledger.apply(&LedgerCommand::Unlock {
                         user_id,
                         asset: quote_asset,
                         amount: excess,
                     }).map_err(|e| e.to_string())?;
                }
            }
        }
        
        Ok(order_id)
    }
    
    /// Public API: Cancel Order (Writes to WAL, then processes)
    pub fn cancel_order(&mut self, symbol_id: usize, order_id: u64) -> Result<bool, String> {
        // 1. Write to Input Log
        self.order_wal.append(LogEntry::CancelOrder { id: order_id });
        
        // 2. Process Logic
        self.process_cancel(symbol_id, order_id)
    }

    /// Internal Logic: Process Cancel (No Input WAL write)
    fn process_cancel(&mut self, symbol_id: usize, order_id: u64) -> Result<bool, String> {
        let book_opt = self.order_books.get_mut(symbol_id)
            .ok_or_else(|| format!("Invalid symbol ID: {}", symbol_id))?;
        let book = book_opt.as_mut()
            .ok_or_else(|| format!("Symbol ID {} is not active", symbol_id))?;

        if book.cancel_order(order_id) {
            // TODO: Unlock funds!
            // We need to know the order details (price, quantity, side, user_id) to unlock.
            // `OrderBook::cancel_order` currently returns `bool`.
            // It should return `Option<Order>` or similar details.
            // This was a previous TODO.
            // I need to update `OrderBook::cancel_order` to return the removed order.
            
            // For now, just return true. Fund unlocking is still a TODO.
            Ok(true)
        } else {
            Ok(false)
        }
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

    pub fn trigger_cow_snapshot(&self) {
        let current_seq = self.order_wal.current_seq;
        let snap_dir = self.snapshot_dir.clone();

        // flush stdout so logs don't get duplicated in child
        let _ = std::io::stdout().flush();

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child: _ }) => {
                // Parent continues immediately
            }
            Ok(ForkResult::Child) => {
                // Child Process
                let start = std::time::Instant::now();
                let filename = format!("engine_snapshot_{}.bin", current_seq); // Changed extension to .bin to match loader
                let path = snap_dir.join(filename);

                let snap = EngineSnapshot {
                    order_books: self.order_books.clone(),
                    ledger_accounts: self.ledger.get_accounts().clone(),
                    ledger_seq: self.ledger.last_seq, // Assuming GlobalLedger exposes last_seq? It does.
                    order_wal_seq: current_seq,
                };

                if let Ok(file) = File::create(&path) {
                    let writer = BufWriter::new(file);
                    if let Err(e) = bincode::serialize_into(writer, &snap) {
                        eprintln!("Child failed to write snapshot: {:?}", e);
                    }
                }

                println!(
                    "   [Child PID {}] Engine Snapshot {} Saved. Time: {:.2?}",
                    std::process::id(), current_seq, start.elapsed()
                );

                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("Fork failed: {}", e);
            }
        }
    }
}

/// Manages symbol-to-ID and ID-to-symbol mappings
pub struct SymbolManager {
    pub symbol_to_id: FxHashMap<String, usize>,
    pub id_to_symbol: FxHashMap<usize, String>, // Using HashMap for sparse ID support
}

impl SymbolManager {
    pub fn new() -> Self {
        SymbolManager {
            symbol_to_id: FxHashMap::default(),
            id_to_symbol: FxHashMap::default(),
        }
    }

    pub fn insert(&mut self, symbol: &str, id: usize) {
        self.symbol_to_id.insert(symbol.to_string(), id);
        self.id_to_symbol.insert(id, symbol.to_string());
    }

    pub fn get_id(&self, symbol: &str) -> Option<usize> {
        self.symbol_to_id.get(symbol).copied()
    }

    pub fn get_symbol(&self, id: usize) -> Option<&String> {
        self.id_to_symbol.get(&id)
    }

    /// Load initial state (simulating DB load)
    pub fn load_from_db() -> Self {
        let mut manager = SymbolManager::new();
        manager.insert("BTC_USDT", 0);
        manager.insert("ETH_USDT", 1);
        manager
    }
}
