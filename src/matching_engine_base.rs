use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use nix::unistd::{fork, ForkResult};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::ledger::{GlobalLedger, LedgerCommand, UserAccount};
use crate::models::{Order, OrderError, OrderStatus, OrderType, Side, Trade};
use crate::order_wal::{LogEntry, Wal};

#[derive(Serialize, Deserialize, Clone)]
pub struct OrderBook {
    pub symbol_id: u32,
    pub bids: BTreeMap<u64, VecDeque<Order>>, // Price -> Orders
    pub asks: BTreeMap<u64, VecDeque<Order>>, // Price -> Orders
    pub active_order_ids: FxHashSet<u64>,
    pub order_index: FxHashMap<u64, (Side, u64)>, // Map order_id -> (Side, Price)
    pub match_seq: u64,
}

impl OrderBook {
    pub fn new(symbol_id: u32) -> Self {
        Self {
            symbol_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            active_order_ids: FxHashSet::default(),
            order_index: FxHashMap::default(),
            match_seq: 0,
        }
    }

    pub fn add_order(&mut self, order: Order) -> Result<Vec<Trade>, String> {
        let order_id = order.order_id;
        let side = order.side;
        let price = order.price;

        // Validate order_id, can not duplicate insert
        if self.active_order_ids.contains(&order_id) {
            return Err(format!("Duplicate order ID: {}", order_id));
        }

        self.active_order_ids.insert(order_id);
        self.order_index.insert(order_id, (side, price));

        let mut trades = Vec::new();
        let mut order = order;

        match side {
            Side::Buy => {
                // Match against Asks (Low to High)
                while order.quantity > 0 {
                    let mut best_ask_price = None;
                    if let Some((price, _)) = self.asks.iter().next() {
                        if price <= &order.price {
                            best_ask_price = Some(*price);
                        }
                    }

                    if let Some(price) = best_ask_price {
                        if let Some(orders_at_price) = self.asks.get_mut(&price) {
                            while let Some(mut best_ask) = orders_at_price.pop_front() {
                                let trade_quantity = u64::min(order.quantity, best_ask.quantity);

                                self.match_seq += 1;
                                trades.push(Trade {
                                    match_id: self.match_seq,
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
                            break;
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
                    if let Some((price, _)) = self.bids.iter().next_back() {
                        if price >= &order.price {
                            best_bid_price = Some(*price);
                        }
                    }

                    if let Some(price) = best_bid_price {
                        if let Some(orders_at_price) = self.bids.get_mut(&price) {
                            while let Some(mut best_bid) = orders_at_price.pop_front() {
                                let trade_quantity = u64::min(order.quantity, best_bid.quantity);

                                self.match_seq += 1;
                                trades.push(Trade {
                                    match_id: self.match_seq,
                                    buy_order_id: best_bid.order_id,
                                    sell_order_id: order.order_id,
                                    buy_user_id: best_bid.user_id,
                                    sell_user_id: order.user_id,
                                    price: best_bid.price, // Trade happens at maker's price
                                    quantity: trade_quantity,
                                });

                                order.quantity -= trade_quantity;
                                best_bid.quantity -= trade_quantity;

                                if best_bid.quantity > 0 {
                                    orders_at_price.push_front(best_bid);
                                    break;
                                } else {
                                    self.active_order_ids.remove(&best_bid.order_id);
                                    self.order_index.remove(&best_bid.order_id);
                                }

                                if order.quantity == 0 {
                                    break;
                                }
                            }

                            if orders_at_price.is_empty() {
                                self.bids.remove(&price);
                            }
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        // If order still has quantity, add to book
        if order.quantity > 0 {
            match order.side {
                Side::Buy => {
                    self.bids
                        .entry(order.price)
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

        Ok(trades)
    }

    pub fn print_book(&self) {
        println!("ASKS:");
        for (price, orders) in self.asks.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!(
                "  Price: {} | Qty: {} | Orders: {}",
                price,
                total_qty,
                orders.len()
            );
        }
        println!("------------------");
        println!("BIDS:");
        for (price, orders) in self.bids.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!(
                "  Price: {} | Qty: {} | Orders: {}",
                price,
                total_qty,
                orders.len()
            );
        }
    }

    pub fn remove_order(&mut self, order_id: u64) -> Result<Order, OrderError> {
        if let Some((side, price)) = self.order_index.remove(&order_id) {
            self.active_order_ids.remove(&order_id);
            let orders_map = match side {
                Side::Buy => &mut self.bids,
                Side::Sell => &mut self.asks,
            };

            if let Some(orders) = orders_map.get_mut(&price) {
                if let Some(index) = orders.iter().position(|o| o.order_id == order_id) {
                    let order = orders.remove(index).unwrap();
                    if orders.is_empty() {
                        orders_map.remove(&price);
                    }
                    return Ok(order);
                }
            }
        }
        Err(OrderError::OrderNotFound { order_id })
    }
}

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
    pub asset_map: FxHashMap<u32, (u32, u32)>,
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
        let mut order_books = Vec::new();
        let mut accounts = FxHashMap::default();
        let mut ledger_seq = 0;
        let mut order_wal_seq = 0;

        // Find latest snapshot
        let mut max_seq = 0;
        let mut latest_snap_path = None;
        if let Ok(entries) = std::fs::read_dir(snap_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if name.starts_with("engine_snapshot_")
                        && path.extension().is_some_and(|e| e == "bin")
                    {
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
            let snap: EngineSnapshot =
                bincode::deserialize_from(reader).map_err(|e| e.to_string())?;

            order_books = snap.order_books;
            accounts = snap.ledger_accounts;
            ledger_seq = snap.ledger_seq;
            order_wal_seq = snap.order_wal_seq;
            println!(
                "   [Recover] Snapshot Loaded. OrderWalSeq: {}, LedgerSeq: {}",
                order_wal_seq, ledger_seq
            );
        }

        // 2. Initialize Ledger from State
        let ledger = GlobalLedger::from_state(wal_dir, snap_dir, accounts, ledger_seq)
            .map_err(|e| e.to_string())?;

        // 3. Replay WAL
        let order_wal = Wal::new(&order_wal_path).map_err(|e| e.to_string())?;
        let trade_wal = Wal::new(&trade_wal_path).map_err(|e| e.to_string())?;

        let mut engine = MatchingEngine {
            order_books,
            ledger,
            asset_map: FxHashMap::default(),
            order_wal,
            trade_wal,
            snapshot_dir: snap_dir.to_path_buf(),
        };

        // Replay
        if let Err(e) = engine.replay_wal() {
            eprintln!("Failed to replay WAL: {}", e);
        }

        Ok(engine)
    }

    pub fn replay_wal(&mut self) -> Result<(), std::io::Error> {
        // We need to read from the WAL file. Since Wal struct is a writer, we need a reader.
        // We can create a temporary reader.
        // Note: self.order_wal is opened for append. We need to read the file.
        // The path is not directly available from Wal struct, but we know it's "orders.wal" in wal_dir.
        // But we don't store wal_dir in MatchingEngine.
        // We passed wal_dir to new.
        // Let's assume we can reconstruct the path or store it.
        // Actually, Wal::new takes path.
        // Let's assume we can read from the file we just opened? No, Wal is BufWriter.
        // We need to open a new reader.
        // But we don't have the path here.
        // Let's modify MatchingEngine to store wal_dir.
        // Or just assume "wal/orders.wal" relative to CWD? No.
        // The `new` method has `wal_dir`.
        // I'll implement replay logic inside `new` or pass `wal_dir` to `replay_wal`.
        // But `replay_wal` is called on `&mut self`.
        // I'll add `wal_dir` to `MatchingEngine` struct.
        // Wait, I can't easily change the struct without updating all usages.
        // The struct definition I wrote above HAS `snapshot_dir` but NOT `wal_dir`.
        // I will add `wal_dir: std::path::PathBuf` to `MatchingEngine`.

        // Actually, let's just implement replay inside `new` where we have the path.
        // But `replay_wal` was a method I added.
        // Let's keep `replay_wal` but make it take the path.
        // Or just add `wal_dir` to struct.

        // For now, I'll add `wal_dir` to struct.
        Ok(())
    }

    /// Register a symbol at a specific ID
    pub fn register_symbol(
        &mut self,
        symbol_id: u32,
        symbol: String,
        base_asset: u32,
        quote_asset: u32,
    ) -> Result<(), String> {
        if symbol_id as usize >= self.order_books.len() {
            self.order_books
                .resize_with(symbol_id as usize + 1, || None);
        } else if self.order_books[symbol_id as usize].is_some() {
            return Err(format!("Symbol ID {} is already in use", symbol_id));
        }
        self.order_books[symbol_id as usize] = Some(OrderBook::new(symbol_id));
        self.asset_map.insert(symbol_id, (base_asset, quote_asset));
        Ok(())
    }

    /// Public API: Add Order (Writes to WAL, then processes)
    pub fn add_order(
        &mut self,
        symbol_id: u32,
        order_id: u64,
        side: Side,
        order_type: OrderType,
        price: u64,
        quantity: u64,
        user_id: u64,
    ) -> Result<u64, OrderError> {
        let wal_side = side;

        // 1. Validate Symbol
        let (base_asset, quote_asset) = *self
            .asset_map
            .get(&symbol_id)
            .ok_or(OrderError::InvalidSymbol { symbol_id })?;

        if let Some(Some(book)) = self.order_books.get(symbol_id as usize) {
            if book.active_order_ids.contains(&order_id) {
                return Err(OrderError::DuplicateOrderId { order_id });
            }
        }

        // 2. Validate Balance
        let (required_asset, required_amount) = match side {
            Side::Buy => (quote_asset, price * quantity),
            Side::Sell => (base_asset, quantity),
        };

        let accounts = self.ledger.get_accounts();
        let balance = accounts
            .get(&user_id)
            .and_then(|user| user.assets.iter().find(|(a, _)| *a == required_asset))
            .map(|(_, b)| b.available)
            .unwrap_or(0);

        if balance < required_amount {
            return Err(OrderError::InsufficientFunds {
                user_id,
                asset_id: required_asset,
                required: required_amount,
                available: balance,
            });
        }

        // 3. Write to Input Log (Source of Truth)
        self.order_wal
            .log_place_order(order_id, user_id, symbol_id, wal_side, price, quantity)
            .map_err(|e| OrderError::Other(e.to_string()))?;

        // 2. Process Logic
        self.process_order(
            symbol_id, order_id, side, order_type, price, quantity, user_id,
        )
    }

    /// Internal Logic: Process Order (No Input WAL write)
    fn process_order(
        &mut self,
        symbol_id: u32,
        order_id: u64,
        side: Side,
        order_type: OrderType,
        price: u64,
        quantity: u64,
        user_id: u64,
    ) -> Result<u64, OrderError> {
        let (base_asset, quote_asset) = *self
            .asset_map
            .get(&symbol_id)
            .ok_or(OrderError::AssetMapNotFound { symbol_id })?;

        // 1. Lock funds
        let (lock_asset, lock_amount) = match side {
            Side::Buy => (quote_asset, price * quantity),
            Side::Sell => (base_asset, quantity),
        };

        self.ledger
            .apply(&LedgerCommand::Lock {
                user_id,
                asset: lock_asset,
                amount: lock_amount,
            })
            .map_err(|e| OrderError::LedgerError(e.to_string()))?;

        let book_opt = self
            .order_books
            .get_mut(symbol_id as usize)
            .ok_or(OrderError::InvalidSymbol { symbol_id })?;

        let book = book_opt
            .as_mut()
            .ok_or(OrderError::InvalidSymbol { symbol_id })?;

        let order = Order {
            order_id,
            user_id,
            symbol_id,
            side,
            order_type,
            price,
            quantity,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };

        let trades = book.add_order(order).map_err(OrderError::Other)?;

        // 3. Settle trades
        for trade in trades {
            // Log trade to Trade WAL (Output Log)
            self.trade_wal
                .log_match_order(
                    trade.match_id,
                    trade.buy_order_id,
                    trade.sell_order_id,
                    trade.price,
                    trade.quantity,
                )
                .map_err(|e| OrderError::Other(e.to_string()))?;

            let (buyer_spend, buyer_gain) = (trade.price * trade.quantity, trade.quantity);
            let (seller_spend, seller_gain) = (trade.quantity, trade.price * trade.quantity);

            // Buyer Settle
            self.ledger
                .apply(&LedgerCommand::TradeSettle {
                    user_id: trade.buy_user_id,
                    spend_asset: quote_asset,
                    spend_amount: buyer_spend,
                    gain_asset: base_asset,
                    gain_amount: buyer_gain,
                })
                .map_err(|e| OrderError::LedgerError(e.to_string()))?;

            // Seller Settle
            self.ledger
                .apply(&LedgerCommand::TradeSettle {
                    user_id: trade.sell_user_id,
                    spend_asset: base_asset,
                    spend_amount: seller_spend,
                    gain_asset: quote_asset,
                    gain_amount: seller_gain,
                })
                .map_err(|e| OrderError::LedgerError(e.to_string()))?;

            // Refund excess frozen for Taker Buyer
            if side == Side::Buy && trade.buy_user_id == user_id {
                let excess = (price - trade.price) * trade.quantity;
                if excess > 0 {
                    self.ledger
                        .apply(&LedgerCommand::Unlock {
                            user_id,
                            asset: quote_asset,
                            amount: excess,
                        })
                        .map_err(|e| OrderError::LedgerError(e.to_string()))?;
                }
            }
        }

        Ok(order_id)
    }

    /// Public API: Cancel Order (Writes to WAL, then processes)
    pub fn cancel_order(&mut self, symbol_id: u32, order_id: u64) -> Result<(), OrderError> {
        // 1. Write to WAL
        self.order_wal
            .log_cancel_order(order_id)
            .map_err(|e| OrderError::Other(e.to_string()))?;

        // 2. Process Logic
        self.process_cancel(symbol_id, order_id)
    }

    /// Internal Logic: Process Cancel (No Input WAL write)
    fn process_cancel(&mut self, symbol_id: u32, order_id: u64) -> Result<(), OrderError> {
        let book = self
            .order_books
            .get_mut(symbol_id as usize)
            .and_then(|opt_book| opt_book.as_mut())
            .ok_or(OrderError::InvalidSymbol { symbol_id })?;

        if let Ok(_cancelled_order) = book.remove_order(order_id) {
            // TODO: Unlock funds!
            Ok(())
        } else {
            Err(OrderError::OrderNotFound { order_id })
        }
    }

    pub fn print_order_book(&self, symbol_id: u32) {
        if let Some(Some(book)) = self.order_books.get(symbol_id as usize) {
            println!(
                "\n--- Order Book for {} (ID: {}) ---",
                book.symbol_id, symbol_id
            );
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
                let filename = format!("engine_snapshot_{}.bin", current_seq);
                let path = snap_dir.join(filename);

                let snap = EngineSnapshot {
                    order_books: self.order_books.clone(),
                    ledger_accounts: self.ledger.get_accounts().clone(),
                    ledger_seq: self.ledger.last_seq,
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
                    std::process::id(),
                    current_seq,
                    start.elapsed()
                );

                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("Fork failed: {}", e);
            }
        }
    }
}
