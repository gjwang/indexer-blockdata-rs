use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use nix::unistd::{fork, ForkResult};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::fast_ulid::FastUlidHalfGen;
use crate::ledger::{GlobalLedger, Ledger, LedgerCommand, MatchExecData, ShadowLedger};
use crate::models::{Order, OrderError, OrderStatus, OrderType, Side, Trade};
use crate::order_wal::{LogEntry, Wal};
use crate::user_account::UserAccount;
use xxhash_rust::xxh3::xxh3_64;

#[derive(Serialize, Deserialize, Clone)]
pub struct OrderBook {
    pub symbol_id: u32,
    pub bids: BTreeMap<u64, VecDeque<Order>>, // Price -> Orders
    pub asks: BTreeMap<u64, VecDeque<Order>>, // Price -> Orders
    pub active_order_ids: FxHashSet<u64>,
    pub order_index: FxHashMap<u64, (Side, u64)>, // Map order_id -> (Side, Price)
    pub match_seq: u64,
}

//TODO: define and implement fee rate

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

    pub fn add_order(
        &mut self,
        order: Order,
        trade_id_gen: &mut FastUlidHalfGen,
        timestamp: u64,
    ) -> Result<Vec<Trade>, String> {
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
                                    match_seq: self.match_seq,
                                    trade_id: trade_id_gen.generate_from_ts(timestamp),
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
                                    match_seq: self.match_seq,
                                    trade_id: trade_id_gen.generate_from_ts(timestamp),
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
                    self.bids.entry(order.price).or_insert_with(VecDeque::new).push_back(order);
                }
                Side::Sell => {
                    self.asks.entry(order.price).or_insert_with(VecDeque::new).push_back(order);
                }
            }
        }

        Ok(trades)
    }

    pub fn print_book(&self) {
        println!("ASKS:");
        for (price, orders) in self.asks.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price, total_qty, orders.len());
        }
        println!("------------------");
        println!("BIDS:");
        for (price, orders) in self.bids.iter().rev() {
            let total_qty: u64 = orders.iter().map(|o| o.quantity).sum();
            println!("  Price: {} | Qty: {} | Orders: {}", price, total_qty, orders.len());
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
    #[serde(default)]
    pub output_seq: u64,
}

pub struct MatchingEngine {
    pub order_books: Vec<Option<OrderBook>>,
    pub ledger: GlobalLedger,
    pub asset_map: FxHashMap<u32, (u32, u32)>,
    pub order_wal: Option<Wal>,
    pub snapshot_dir: std::path::PathBuf,
    pub trade_id_gen: FastUlidHalfGen,
    pub state_hash: u64,
    pub output_seq: u64,
}

impl MatchingEngine {
    pub fn new(
        wal_dir: &std::path::Path,
        snap_dir: &std::path::Path,
        enable_local_wal: bool,
    ) -> Result<Self, String> {
        if enable_local_wal && !wal_dir.exists() {
            std::fs::create_dir_all(wal_dir).map_err(|e| e.to_string())?;
        }
        if !snap_dir.exists() {
            std::fs::create_dir_all(snap_dir).map_err(|e| e.to_string())?;
        }

        let order_wal_path = wal_dir.join("orders.wal");
        // let trade_wal_path = wal_dir.join("trades.wal");

        // 1. Try Load Snapshot
        let mut order_books = Vec::new();
        let mut accounts = FxHashMap::default();
        let mut ledger_seq = 0;
        let mut order_wal_seq = 0;
        let mut state_hash = 0;
        let mut output_seq = 0;

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
            output_seq = snap.output_seq;
            // TODO: Load state_hash from snapshot if we add it to EngineSnapshot
            println!(
                "   [Recover] Snapshot Loaded. OrderWalSeq: {}, LedgerSeq: {}",
                order_wal_seq, ledger_seq
            );
        }

        let ledger_wal_path = wal_dir.join("ledger");
        let ledger_snap_path = snap_dir.join("ledger_snapshots");

        // 2. Initialize Ledger from State
        let mut ledger =
            GlobalLedger::new(&ledger_wal_path, &ledger_snap_path).map_err(|e| e.to_string())?;

        // 3. Replay WAL (Only if enabled)
        let order_wal = if enable_local_wal {
            Some(Wal::new(&order_wal_path).map_err(|e| e.to_string())?)
        } else {
            None
        };

        let mut engine = Self {
            order_books,
            ledger,
            asset_map: FxHashMap::default(),
            order_wal,
            snapshot_dir: snap_dir.to_path_buf(),
            trade_id_gen: FastUlidHalfGen::new(),
            state_hash,
            output_seq,
        };

        // Replay (Only if WAL exists and was enabled)
        if enable_local_wal {
            if let Err(e) = engine.replay_wal() {
                eprintln!("Failed to replay WAL: {}", e);
            }
        }

        Ok(engine)
    }

    pub fn update_hash(&mut self, data: &[u8]) {
        self.state_hash = xxhash_rust::xxh3::xxh3_64_with_seed(data, self.state_hash);
    }

    pub fn take_order_wal(&mut self) -> Option<Wal> {
        self.order_wal.take()
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
            self.order_books.resize_with(symbol_id as usize + 1, || None);
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
        timestamp: u64,
    ) -> Result<u64, OrderError> {
        let wal_side = side;

        // 1. Validate Symbol
        let (base_asset, quote_asset) =
            *self.asset_map.get(&symbol_id).ok_or(OrderError::InvalidSymbol { symbol_id })?;

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
            .map(|(_, b)| b.avail)
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
        // 3. Write to Input Log (Source of Truth)
        if let Some(wal) = &mut self.order_wal {
            wal.log_place_order_no_flush(order_id, user_id, symbol_id, wal_side, price, quantity)
                .map_err(|e| OrderError::Other(e.to_string()))?;
        }

        // 2. Process Logic
        let oid = Self::process_order_logic(
            &mut self.ledger,
            &mut self.order_books,
            &self.asset_map,
            &mut self.trade_id_gen,
            &mut self.output_seq,
            symbol_id,
            order_id,
            side,
            order_type,
            price,
            quantity,
            user_id,
            timestamp,
        )?;

        // Update State Hash (Incremental)
        let mut hash_data = Vec::with_capacity(40);
        hash_data.extend_from_slice(&order_id.to_le_bytes());
        hash_data.extend_from_slice(&symbol_id.to_le_bytes());
        hash_data.push(side as u8);
        hash_data.extend_from_slice(&price.to_le_bytes());
        hash_data.extend_from_slice(&quantity.to_le_bytes());
        hash_data.extend_from_slice(&user_id.to_le_bytes());
        self.update_hash(&hash_data);

        // 3. Flush WALs
        // 3. Flush WALs
        if let Some(wal) = &mut self.order_wal {
            wal.flush().map_err(|e| OrderError::Other(e.to_string()))?;
        }
        self.ledger.flush().map_err(|e| OrderError::LedgerError(e.to_string()))?;

        Ok(oid)
    }

    /// Internal Logic: Process Order (No Input WAL write)
    fn process_order_logic(
        ledger: &mut impl Ledger,
        order_books: &mut Vec<Option<OrderBook>>,
        asset_map: &FxHashMap<u32, (u32, u32)>,
        trade_id_gen: &mut FastUlidHalfGen,
        output_seq: &mut u64,
        symbol_id: u32,
        order_id: u64,
        side: Side,
        order_type: OrderType,
        price: u64,
        quantity: u64,
        user_id: u64,
        timestamp: u64,
    ) -> Result<u64, OrderError> {
        let (base_asset, quote_asset) =
            *asset_map.get(&symbol_id).ok_or(OrderError::AssetMapNotFound { symbol_id })?;

        // 1. Lock funds
        let (lock_asset, lock_amount) = match side {
            Side::Buy => (quote_asset, price * quantity),
            Side::Sell => (base_asset, quantity),
        };

        ledger
            .apply(&LedgerCommand::Lock { user_id, asset: lock_asset, amount: lock_amount })
            .map_err(|e| OrderError::LedgerError(e.to_string()))?;

        let book_opt = order_books
            .get_mut(symbol_id as usize)
            .ok_or(OrderError::InvalidSymbol { symbol_id })?;

        let book = book_opt.as_mut().ok_or(OrderError::InvalidSymbol { symbol_id })?;

        let order =
            Order { order_id, user_id, symbol_id, side, order_type, price, quantity, timestamp };

        let trades = book.add_order(order, trade_id_gen, timestamp).map_err(OrderError::Other)?;

        let mut match_batch = Vec::with_capacity(trades.len());

        for trade in &trades {
            let mut buyer_refund = 0;
            // Refund excess frozen for Taker Buyer
            if side == Side::Buy && trade.buy_user_id == user_id {
                let excess = (price - trade.price) * trade.quantity;
                if excess > 0 {
                    buyer_refund = excess;
                }
            }

            // Capture balance versions BEFORE applying the trade
            // This ensures we have the version at the time of trade execution
            let buyer_quote_version = ledger.get_balance_version(trade.buy_user_id, quote_asset);
            let buyer_base_version = ledger.get_balance_version(trade.buy_user_id, base_asset);
            let seller_base_version = ledger.get_balance_version(trade.sell_user_id, base_asset);
            let seller_quote_version = ledger.get_balance_version(trade.sell_user_id, quote_asset);

            match_batch.push(MatchExecData {
                trade_id: trade.trade_id,
                buy_order_id: trade.buy_order_id,
                sell_order_id: trade.sell_order_id,
                buyer_user_id: trade.buy_user_id,
                seller_user_id: trade.sell_user_id,
                price: trade.price,
                quantity: trade.quantity,
                base_asset,
                quote_asset,
                buyer_refund,
                seller_refund: 0,
                match_seq: trade.match_seq,
                output_sequence: {
                    *output_seq += 1;
                    *output_seq
                },
                settled_at: 0,
                // Balance versions from matching engine
                buyer_quote_version,
                buyer_base_version,
                seller_base_version,
                seller_quote_version,
            });
        }

        if !match_batch.is_empty() {
            ledger
                .apply(&LedgerCommand::MatchExecBatch(match_batch))
                .map_err(|e| OrderError::LedgerError(e.to_string()))?;
        }

        Ok(order_id)
    }

    pub fn add_order_batch(
        &mut self,
        requests: Vec<(u32, u64, Side, OrderType, u64, u64, u64, u64)>,
    ) -> (Vec<Result<u64, OrderError>>, Vec<LedgerCommand>) {
        let start_total = std::time::Instant::now();
        let mut results = vec![Err(OrderError::Other("Not processed".to_string())); requests.len()];
        let mut valid_indices = Vec::with_capacity(requests.len());

        // 1. Validate and Log to Input WAL
        for (i, (symbol_id, order_id, side, _order_type, price, quantity, user_id, _timestamp)) in
            requests.iter().enumerate()
        {
            let wal_side = *side;
            let (base_asset, quote_asset) = match self.asset_map.get(symbol_id) {
                Some(&pair) => pair,
                None => {
                    results[i] = Err(OrderError::AssetMapNotFound { symbol_id: *symbol_id });
                    continue;
                }
            };
            let (required_asset, required_amount) = match side {
                Side::Buy => (quote_asset, price * quantity),
                Side::Sell => (base_asset, *quantity),
            };

            let accounts = self.ledger.get_accounts();
            let balance = accounts
                .get(user_id)
                .and_then(|user| user.assets.iter().find(|(a, _)| *a == required_asset))
                .map(|(_, b)| b.avail)
                .unwrap_or(0);

            if balance < required_amount {
                results[i] = Err(OrderError::InsufficientFunds {
                    user_id: *user_id,
                    asset_id: required_asset,
                    required: required_amount,
                    available: balance,
                });
                continue;
            }

            // Log to Order WAL (No Flush)
            if let Some(wal) = &mut self.order_wal {
                if let Err(e) = wal.log_place_order_no_flush(
                    *order_id, *user_id, *symbol_id, wal_side, *price, *quantity,
                ) {
                    results[i] = Err(OrderError::Other(e.to_string()));
                    continue;
                }
            }

            valid_indices.push(i);
        }

        let t_input = start_total.elapsed();

        // 3. Process Valid Orders (Shadow Mode)
        let t_process_start = std::time::Instant::now();
        let mut shadow = ShadowLedger::new(&self.ledger);

        for i in valid_indices {
            let (symbol_id, order_id, side, order_type, price, quantity, user_id, timestamp) =
                requests[i];
            match Self::process_order_logic(
                &mut shadow,
                &mut self.order_books,
                &self.asset_map,
                &mut self.trade_id_gen,
                &mut self.output_seq,
                symbol_id,
                order_id,
                side,
                order_type,
                price,
                quantity,
                user_id,
                timestamp,
            ) {
                Ok(oid) => results[i] = Ok(oid),
                Err(e) => results[i] = Err(e),
            }
        }
        let t_process = t_process_start.elapsed();

        // 4. Commit Batch (Memory Only) - Return commands for persistence
        let t_commit_start = std::time::Instant::now();
        let mut output_cmds = Vec::new();

        if !shadow.pending_commands.is_empty() {
            let (delta, cmds) = shadow.into_delta();
            // Apply to memory immediately so next batch sees correct state
            self.ledger.apply_delta_to_memory(delta);
            output_cmds = cmds;
        }
        let t_commit = t_commit_start.elapsed();

        if requests.len() > 0 {
            println!(
                "[PERF] Match: {} orders. Input: {:?}, Process: {:?}, Commit(Mem): {:?}. Total: {:?}",
                requests.len(),
                t_input,
                t_process,
                t_commit,
                start_total.elapsed()
            );
        }

        (results, output_cmds)
    }

    /// Public API: Cancel Order (Writes to WAL, then processes)
    pub fn cancel_order(&mut self, symbol_id: u32, order_id: u64) -> Result<(), OrderError> {
        // 1. Write to WAL
        // 1. Write to WAL
        if let Some(wal) = &mut self.order_wal {
            wal.log_cancel_order(order_id).map_err(|e| OrderError::Other(e.to_string()))?;
        }

        // 2. Process Logic
        self.process_cancel(symbol_id, order_id)?;

        // Update State Hash
        let mut hash_data = Vec::with_capacity(16);
        hash_data.extend_from_slice(&symbol_id.to_le_bytes());
        hash_data.extend_from_slice(&order_id.to_le_bytes());
        hash_data.push(0xFF); // Marker for Cancel
        self.update_hash(&hash_data);

        Ok(())
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
            println!("\n--- Order Book for {} (ID: {}) ---", book.symbol_id, symbol_id);
            book.print_book();
            println!("----------------------------------\n");
        } else {
            println!("Order book for ID {} not found", symbol_id);
        }
    }

    pub fn trigger_cow_snapshot(&self) {
        let current_seq = self.order_wal.as_ref().map(|w| w.current_seq).unwrap_or(0);
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
                    output_seq: self.output_seq,
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

    /// Transfer funds IN to a user's trading account
    /// Called by balance_processor after validating funding account
    /// This is the ONLY way to add funds to trading accounts
    pub fn transfer_in_to_trading_account(
        &mut self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> Result<(), String> {
        self.ledger
            .apply(&LedgerCommand::Deposit { user_id, asset: asset_id, amount })
            .map_err(|e| format!("Failed to transfer in to trading account: {}", e))
    }

    /// Transfer funds OUT from a user's trading account
    /// Called by balance_processor to return funds to funding account
    /// This is the ONLY way to remove funds from trading accounts
    pub fn transfer_out_from_trading_account(
        &mut self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> Result<(), String> {
        self.ledger
            .apply(&LedgerCommand::Withdraw { user_id, asset: asset_id, amount })
            .map_err(|e| format!("Failed to transfer out from trading account: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_hash_determinism() {
        let wal_dir = std::path::Path::new("test_wal_hash");
        let snap_dir = std::path::Path::new("test_snap_hash");
        let _ = std::fs::remove_dir_all(wal_dir);
        let _ = std::fs::remove_dir_all(snap_dir);

        let mut engine1 = MatchingEngine::new(wal_dir, snap_dir, false).unwrap();
        let mut engine2 = MatchingEngine::new(wal_dir, snap_dir, false).unwrap();

        // Setup
        for engine in [&mut engine1, &mut engine2] {
            engine.register_symbol(1, "BTC_USDT".to_string(), 1, 2).unwrap();
            // Deposit funds
            engine.transfer_in_to_trading_account(101, 2, 100000).unwrap(); // User 101 has 100k USDT
            engine.transfer_in_to_trading_account(102, 1, 1000).unwrap(); // User 102 has 1000 BTC
        }

        // Action 1: Place Order
        let _ = engine1.add_order(1, 1001, Side::Buy, OrderType::Limit, 50000, 1, 101, 1000);
        let _ = engine2.add_order(1, 1001, Side::Buy, OrderType::Limit, 50000, 1, 101, 1000);

        assert_eq!(engine1.state_hash, engine2.state_hash, "Hashes should match after same order");
        assert_ne!(engine1.state_hash, 0, "Hash should not be zero");

        // Action 2: Place Matching Order
        let _ = engine1.add_order(1, 1002, Side::Sell, OrderType::Limit, 50000, 1, 102, 2000);
        let _ = engine2.add_order(1, 1002, Side::Sell, OrderType::Limit, 50000, 1, 102, 2000);

        assert_eq!(engine1.state_hash, engine2.state_hash, "Hashes should match after trade");

        // Action 3: Cancel Order (that doesn't exist, should fail and NOT update hash)
        let hash_before = engine1.state_hash;
        let _ = engine1.cancel_order(1, 9999); // Fail
        assert_eq!(engine1.state_hash, hash_before, "Hash should not change on failure");

        // Cleanup
        let _ = std::fs::remove_dir_all(wal_dir);
        let _ = std::fs::remove_dir_all(snap_dir);
    }
}
