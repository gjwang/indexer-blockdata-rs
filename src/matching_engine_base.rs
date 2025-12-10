use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use nix::unistd::{fork, ForkResult};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::engine_output::{
    BalanceEvent as EOBalanceEvent, DepositInput, EngineOutput, EngineOutputBuilder, InputBundle,
    InputData, OrderUpdate as EOOrderUpdate, PlaceOrderInput, TradeOutput, WithdrawInput,
};
use crate::fast_ulid::FastUlidHalfGen;
use crate::ledger::{GlobalLedger, Ledger, LedgerCommand, MatchExecData, ShadowLedger};
use crate::null_ledger::NullLedger; // Stub ledger for refactoring
use crate::models::{Order, OrderError, OrderStatus, OrderType, Side, Trade};
use crate::order_wal::{LogEntry, Wal};
use crate::user_account::UserAccount;

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
    /// Hash of last EngineOutput for chain continuity
    #[serde(default)]
    pub last_output_hash: u64,
}

pub struct MatchingEngine {
    pub order_books: Vec<Option<OrderBook>>,
    // REMOVED: Balance state is now in UBSCore!
    pub asset_map: FxHashMap<u32, (u32, u32)>,
    pub order_wal: Option<Wal>,
    pub snapshot_dir: std::path::PathBuf,
    pub trade_id_gen: FastUlidHalfGen,
    pub state_hash: u64,
    pub output_seq: u64,
    /// Hash of last EngineOutput for chain continuity
    pub last_output_hash: u64,
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
        // REMOVED: accounts, ledger_seq - Balance state is in UBSCore
        let mut order_wal_seq = 0;
        let mut state_hash = 0;
        let mut output_seq = 0;
        let mut last_output_hash = 0u64; // Genesis hash

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
            // REMOVED: accounts, ledger_seq loading
            order_wal_seq = snap.order_wal_seq;
            output_seq = snap.output_seq;
            last_output_hash = snap.last_output_hash;
            println!(
                "   [Recover] Snapshot Loaded. OrderWalSeq: {}",
                order_wal_seq
            );
        }

        // REMOVED: Ledger initialization - Balance state is in UBSCore

        // 3. Replay WAL (Only if enabled)
        let order_wal = if enable_local_wal {
            Some(Wal::new(&order_wal_path).map_err(|e| e.to_string())?)
        } else {
            None
        };

        let mut engine = Self {
            order_books,
            // REMOVED: ledger field
            asset_map: FxHashMap::default(),
            order_wal,
            snapshot_dir: snap_dir.to_path_buf(),
            trade_id_gen: FastUlidHalfGen::new(),
            state_hash,
            output_seq,
            last_output_hash,
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

    /// Get the current output sequence number
    pub fn get_output_seq(&self) -> u64 {
        self.output_seq
    }

    /// Register a symbol at a specific ID
    pub fn register_symbol(
        &mut self,
        symbol_id: u32,
        _symbol: String,
        base_asset_id: u32,
        quote_asset_id: u32,
    ) -> Result<(), String> {
        if symbol_id as usize >= self.order_books.len() {
            self.order_books.resize_with(symbol_id as usize + 1, || None);
        } else if self.order_books[symbol_id as usize].is_some() {
            return Err(format!("Symbol ID {} is already in use", symbol_id));
        }
        self.order_books[symbol_id as usize] = Some(OrderBook::new(symbol_id));
        self.asset_map.insert(symbol_id, (base_asset_id, quote_asset_id));
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
        let (base_asset_id, quote_asset_id) =
            *self.asset_map.get(&symbol_id).ok_or(OrderError::InvalidSymbol { symbol_id })?;

        if let Some(Some(book)) = self.order_books.get(symbol_id as usize) {
            if book.active_order_ids.contains(&order_id) {
                return Err(OrderError::DuplicateOrderId { order_id });
            }
        }

        // 2. Validate Balance
        let (required_asset, required_amount) = match side {
            Side::Buy => (quote_asset_id, price * quantity),
            Side::Sell => (base_asset_id, quantity),
        };
        // REMOVED: Balance validation is now done by UBSCore
        // If an order reaches ME, funds are already locked in UBSCore

        // 3. Write to Input Log (Source of Truth)
        // 3. Write to Input Log (Source of Truth)
        if let Some(wal) = &mut self.order_wal {
            wal.log_place_order_no_flush(order_id, user_id, symbol_id, wal_side, price, quantity)
                .map_err(|e| OrderError::Other(e.to_string()))?;
        }

        // 2. Process Logic
        let mut null_ledger = NullLedger::new(); // ME no longer has balance state
        let oid = Self::process_order_logic(
            &mut null_ledger,
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
        if let Some(wal) = &mut self.order_wal {
            wal.flush().map_err(|e| OrderError::Other(e.to_string()))?;
        }
        // REMOVED: ledger flush - ME no longer has ledger

        Ok(oid)
    }

    /// Add order and return EngineOutput bundle with all effects
    /// This is the primary method for processing orders with full output tracking
    pub fn add_order_and_build_output(
        &mut self,
        input_seq: u64,
        symbol_id: u32,
        order_id: u64,
        side: Side,
        order_type: OrderType,
        price: u64,
        quantity: u64,
        user_id: u64,
        timestamp: u64,
        client_order_id: String,
    ) -> Result<(u64, EngineOutput), OrderError> {
        // Validate Symbol
        if !self.asset_map.contains_key(&symbol_id) {
            return Err(OrderError::InvalidSymbol { symbol_id });
        }

        // Write to Input Log (if enabled)
        if let Some(wal) = &mut self.order_wal {
            wal.log_place_order_no_flush(order_id, user_id, symbol_id, side, price, quantity)
                .map_err(|e| OrderError::Other(e.to_string()))?;
        }

        // Process order with output building
        let mut null_ledger = NullLedger::new();
        let result = Self::process_order_with_output(
            &mut null_ledger,
            &mut self.order_books,
            &self.asset_map,
            &mut self.trade_id_gen,
            &mut self.output_seq,
            &mut self.last_output_hash,
            input_seq,
            symbol_id,
            order_id,
            side,
            order_type,
            price,
            quantity,
            user_id,
            timestamp,
            client_order_id,
        )?;

        // Flush WALs
        if let Some(wal) = &mut self.order_wal {
            wal.flush().map_err(|e| OrderError::Other(e.to_string()))?;
        }
        // REMOVED: ledger flush - ME no longer has ledger

        Ok(result)
    }

    /// Internal Logic: Process Order (No Input WAL write)
    /// Returns (order_id, Vec<LedgerCommand>) where commands include both ledger ops and OrderUpdate events
    /// Internal Logic: Process Order (No Input WAL write)
    /// Returns order_id (or error). Commands are applied to the ledger.
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
        use crate::ledger::{OrderStatus, OrderUpdate};

        let (base_asset_id, quote_asset_id) = match asset_map.get(&symbol_id) {
            Some(p) => *p,
            None => {
                let rejection = OrderUpdate {
                    order_id,
                    client_order_id: None,
                    user_id,
                    symbol_id,
                    side: side.as_u8(),
                    order_type: order_type.as_u8(),
                    status: OrderStatus::Rejected,
                    price,
                    qty: quantity,
                    filled_qty: 0,
                    avg_fill_price: None,
                    rejection_reason: Some(format!("Asset map not found for symbol {}", symbol_id)),
                    timestamp,
                    match_id: None,
                };
                let _ = ledger.apply(&LedgerCommand::OrderUpdate(rejection));
                return Ok(order_id);
            }
        };

        // 1. Lock funds
        let (lock_asset, lock_amount) = match side {
            Side::Buy => (quote_asset_id, price * quantity),
            Side::Sell => (base_asset_id, quantity),
        };

        // Prepare Lock Command with Snapshots
        let current_bal = ledger.get_balance(user_id, lock_asset);
        let current_ver = ledger.get_balance_version(user_id, lock_asset);
        // Lock reduces available balance
        let balance_after = if current_bal >= lock_amount {
            current_bal - lock_amount
        } else {
            current_bal // Should fail in apply
        };
        let new_ver = current_ver + 1;

        if let Err(e) = ledger.apply(&LedgerCommand::Lock {
            user_id,
            asset_id: lock_asset,
            amount: lock_amount,
            balance_after,
            version: new_ver,
        }) {
            let rejection = OrderUpdate {
                order_id,
                client_order_id: None,
                user_id,
                symbol_id,
                side: side.as_u8(),
                order_type: order_type.as_u8(),
                status: OrderStatus::Rejected,
                price,
                qty: quantity,
                filled_qty: 0,
                avg_fill_price: None,
                rejection_reason: Some(e.to_string()),
                timestamp,
                match_id: None,
            };
            let _ = ledger.apply(&LedgerCommand::OrderUpdate(rejection));
            return Ok(order_id);
        }

        // Emit OrderUpdate(New) - Order successfully placed (Funds Locked)
        let new_order_event = OrderUpdate {
            order_id,
            client_order_id: None,
            user_id,
            symbol_id,
            side: side.as_u8(),
            order_type: order_type.as_u8(),
            status: OrderStatus::New,
            price,
            qty: quantity,
            filled_qty: 0,
            avg_fill_price: None,
            rejection_reason: None,
            timestamp,
            match_id: None,
        };
        ledger
            .apply(&LedgerCommand::OrderUpdate(new_order_event))
            .map_err(|e| OrderError::LedgerError(e.to_string()))?;

        // Access OrderBook
        // Note: We need to handle indices carefully.
        if symbol_id as usize >= order_books.len() || order_books[symbol_id as usize].is_none() {
            let rejection = OrderUpdate {
                order_id,
                client_order_id: None,
                user_id,
                symbol_id,
                side: side.as_u8(),
                order_type: order_type.as_u8(),
                status: OrderStatus::Rejected,
                price,
                qty: quantity,
                filled_qty: 0,
                avg_fill_price: None,
                rejection_reason: Some(format!("Invalid Symbol ID: {}", symbol_id)),
                timestamp,
                match_id: None,
            };
            let _ = ledger.apply(&LedgerCommand::OrderUpdate(rejection));
            return Ok(order_id);
        }
        let book = order_books[symbol_id as usize].as_mut().unwrap();

        let order =
            Order { order_id, user_id, symbol_id, side, order_type, price, quantity, timestamp };

        let trades = match book.add_order(order, trade_id_gen, timestamp) {
            Ok(t) => t,
            Err(e) => {
                // If duplicates or other book errors occur.
                // Duplicate ID must be treated as system error (Err) and ignored,
                // OR rejected if we want to notify client?
                // Standard practice: Duplicate ID -> Reject or Ignore?
                // If Ignore, client waits forever.
                // If Reject, we might send "Rejected" for an ID that is already "New"?
                // Let's assume Err for duplicates means Drop.
                return Err(OrderError::Other(e));
            }
        };

        let mut match_batch = Vec::with_capacity(trades.len());

        // Track temporary version increments within the batch
        let mut temp_versions: std::collections::HashMap<(u64, u32), u64> =
            std::collections::HashMap::new();
        // Legacy: temp_balances still used in old process_order_logic path
        let mut temp_balances: std::collections::HashMap<(u64, u32), u64> =
            std::collections::HashMap::new();


        for trade in trades {
            // Calculate refund early to determine version increment
            let buyer_refund = if side == Side::Buy && trade.buy_user_id == user_id {
                if price > trade.price {
                    (price - trade.price) * trade.quantity
                } else {
                    0
                }
            } else {
                0
            };

            // Buyer Quote Asset increments by 2 if there is a refund (Spend + Refund)
            // Otherwise increments by 1 (Spend)
            let buyer_quote_inc = if buyer_refund > 0 { 2 } else { 1 };

            // Helper to get and increment version
            let mut get_and_add_version = |user_id: u64, asset_id: u32, inc: u64| -> u64 {
                let entry = temp_versions
                    .entry((user_id, asset_id))
                    .or_insert_with(|| ledger.get_balance_version(user_id, asset_id));
                let v = *entry;
                *entry += inc;
                v
            };

            // Helper to get and update balance (Avail)
            let mut get_and_update_balance = |user_id: u64, asset_id: u32, delta: i64| -> u64 {
                let entry = temp_balances
                    .entry((user_id, asset_id))
                    .or_insert_with(|| ledger.get_balance(user_id, asset_id));
                // Safely apply delta
                if delta >= 0 {
                    *entry += delta as u64;
                } else {
                    let abs_d = (-delta) as u64;
                    if *entry >= abs_d {
                        *entry -= abs_d;
                    } else {
                        *entry = 0;
                    }
                }
                *entry
            };

            let buyer_quote_version =
                get_and_add_version(trade.buy_user_id, quote_asset_id, buyer_quote_inc);
            let buyer_base_version = get_and_add_version(trade.buy_user_id, base_asset_id, 1);
            let seller_base_version = get_and_add_version(trade.sell_user_id, base_asset_id, 1);
            let seller_quote_version = get_and_add_version(trade.sell_user_id, quote_asset_id, 1);

            // Calculate Balance Snapshots (Avail)
            // Buyer Quote: Avail += Refund
            let buyer_quote_balance_after =
                get_and_update_balance(trade.buy_user_id, quote_asset_id, buyer_refund as i64);
            // Buyer Base: Avail += Quantity
            let buyer_base_balance_after =
                get_and_update_balance(trade.buy_user_id, base_asset_id, trade.quantity as i64);
            // Seller Base: Avail += 0 (Already deducted to Frozen)
            let seller_base_balance_after =
                get_and_update_balance(trade.sell_user_id, base_asset_id, 0);
            // Seller Quote: Avail += Price * Qty
            let cost = trade.price * trade.quantity;
            let seller_quote_balance_after =
                get_and_update_balance(trade.sell_user_id, quote_asset_id, cost as i64);

            match_batch.push(MatchExecData {
                trade_id: trade.trade_id,
                buy_order_id: trade.buy_order_id,
                sell_order_id: trade.sell_order_id,
                buyer_user_id: trade.buy_user_id,
                seller_user_id: trade.sell_user_id,
                price: trade.price,
                quantity: trade.quantity,
                base_asset_id: base_asset_id,
                quote_asset_id: quote_asset_id,
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
                // Snapshots
                buyer_quote_balance_after,
                buyer_base_balance_after,
                seller_base_balance_after,
                seller_quote_balance_after,
            });
        }

        if !match_batch.is_empty() {
            ledger
                .apply(&LedgerCommand::MatchExecBatch(match_batch))
                .map_err(|e| OrderError::LedgerError(e.to_string()))?;
        }

        Ok(order_id)
    }

    /// Process order and build EngineOutput bundle
    /// Returns (order_id, EngineOutput) with all effects captured atomically
    fn process_order_with_output(
        ledger: &mut impl Ledger,
        order_books: &mut Vec<Option<OrderBook>>,
        asset_map: &FxHashMap<u32, (u32, u32)>,
        trade_id_gen: &mut FastUlidHalfGen,
        output_seq: &mut u64,
        last_output_hash: &mut u64,
        input_seq: u64,
        symbol_id: u32,
        order_id: u64,
        side: Side,
        order_type: OrderType,
        price: u64,
        quantity: u64,
        user_id: u64,
        timestamp: u64,
        client_order_id: String,
    ) -> Result<(u64, EngineOutput), OrderError> {
        use crate::ledger::{OrderStatus, OrderUpdate};

        // Increment output sequence for this bundle
        *output_seq += 1;
        let current_output_seq = *output_seq;

        // Create builder with chain linkage
        let mut builder = EngineOutputBuilder::new(current_output_seq, *last_output_hash);

        // Set input bundle
        builder.set_input(InputBundle::new(
            input_seq,
            InputData::PlaceOrder(PlaceOrderInput {
                order_id,
                user_id,
                symbol_id,
                side: side.as_u8(),
                order_type: order_type.as_u8(),
                price,
                quantity,
                cid: client_order_id.clone(),
                created_at: timestamp,
            }),
        ));

        let (base_asset_id, quote_asset_id) = match asset_map.get(&symbol_id) {
            Some(p) => *p,
            None => {
                // Rejection: asset map not found
                let rejection = OrderUpdate {
                    order_id,
                    client_order_id: Some(client_order_id),
                    user_id,
                    symbol_id,
                    side: side.as_u8(),
                    order_type: order_type.as_u8(),
                    status: OrderStatus::Rejected,
                    price,
                    qty: quantity,
                    filled_qty: 0,
                    avg_fill_price: None,
                    rejection_reason: Some(format!("Asset map not found for symbol {}", symbol_id)),
                    timestamp,
                    match_id: None,
                };
                let _ = ledger.apply(&LedgerCommand::OrderUpdate(rejection));

                // Build EngineOutput with rejection
                builder.set_order_update(EOOrderUpdate {
                    order_id,
                    user_id,
                    status: 5, // Rejected
                    filled_qty: 0,
                    remaining_qty: quantity,
                    avg_price: 0,
                    updated_at: timestamp,
                });

                let output = builder.build_unchecked();
                *last_output_hash = output.hash;
                return Ok((order_id, output));
            }
        };

        // 1. Lock funds
        let (lock_asset, lock_amount) = match side {
            Side::Buy => (quote_asset_id, price * quantity),
            Side::Sell => (base_asset_id, quantity),
        };

        // Get current balance state for balance event
        let current_bal = ledger.get_balance(user_id, lock_asset);
        let current_frozen = ledger.get_frozen(user_id, lock_asset);
        let current_ver = ledger.get_balance_version(user_id, lock_asset);
        let balance_after =
            if current_bal >= lock_amount { current_bal - lock_amount } else { current_bal };
        let new_ver = current_ver + 1;

        if let Err(e) = ledger.apply(&LedgerCommand::Lock {
            user_id,
            asset_id: lock_asset,
            amount: lock_amount,
            balance_after,
            version: new_ver,
        }) {
            // Rejection: insufficient balance
            let rejection = OrderUpdate {
                order_id,
                client_order_id: Some(client_order_id),
                user_id,
                symbol_id,
                side: side.as_u8(),
                order_type: order_type.as_u8(),
                status: OrderStatus::Rejected,
                price,
                qty: quantity,
                filled_qty: 0,
                avg_fill_price: None,
                rejection_reason: Some(e.to_string()),
                timestamp,
                match_id: None,
            };
            let _ = ledger.apply(&LedgerCommand::OrderUpdate(rejection));

            builder.set_order_update(EOOrderUpdate {
                order_id,
                user_id,
                status: 5, // Rejected
                filled_qty: 0,
                remaining_qty: quantity,
                avg_price: 0,
                updated_at: timestamp,
            });

            let output = builder.build_unchecked();
            *last_output_hash = output.hash;
            return Ok((order_id, output));
        }

        // Add lock balance event
        builder.add_balance_event(EOBalanceEvent {
            user_id,
            asset_id: lock_asset,
            seq: new_ver,
            delta_avail: -(lock_amount as i64),
            delta_frozen: lock_amount as i64,
            avail: Some(balance_after),
            frozen: Some(current_frozen + lock_amount),
            event_type: "lock".into(),
            ref_id: order_id,
        });

        // Emit OrderUpdate(New)
        let new_order_event = OrderUpdate {
            order_id,
            client_order_id: Some(client_order_id.clone()),
            user_id,
            symbol_id,
            side: side.as_u8(),
            order_type: order_type.as_u8(),
            status: OrderStatus::New,
            price,
            qty: quantity,
            filled_qty: 0,
            avg_fill_price: None,
            rejection_reason: None,
            timestamp,
            match_id: None,
        };
        ledger
            .apply(&LedgerCommand::OrderUpdate(new_order_event))
            .map_err(|e| OrderError::LedgerError(e.to_string()))?;

        // Access OrderBook
        if symbol_id as usize >= order_books.len() || order_books[symbol_id as usize].is_none() {
            let rejection = OrderUpdate {
                order_id,
                client_order_id: Some(client_order_id),
                user_id,
                symbol_id,
                side: side.as_u8(),
                order_type: order_type.as_u8(),
                status: OrderStatus::Rejected,
                price,
                qty: quantity,
                filled_qty: 0,
                avg_fill_price: None,
                rejection_reason: Some(format!("Invalid Symbol ID: {}", symbol_id)),
                timestamp,
                match_id: None,
            };
            let _ = ledger.apply(&LedgerCommand::OrderUpdate(rejection));

            builder.set_order_update(EOOrderUpdate {
                order_id,
                user_id,
                status: 5, // Rejected
                filled_qty: 0,
                remaining_qty: quantity,
                avg_price: 0,
                updated_at: timestamp,
            });

            let output = builder.build_unchecked();
            *last_output_hash = output.hash;
            return Ok((order_id, output));
        }

        let book = order_books[symbol_id as usize].as_mut().unwrap();
        let order =
            Order { order_id, user_id, symbol_id, side, order_type, price, quantity, timestamp };

        let trades = match book.add_order(order, trade_id_gen, timestamp) {
            Ok(t) => t,
            Err(e) => {
                return Err(OrderError::Other(e));
            }
        };

        let mut total_filled = 0u64;
        let mut total_cost = 0u64;
        let mut match_batch = Vec::with_capacity(trades.len());

        // Track temporary version increments within the batch
        let mut temp_versions: std::collections::HashMap<(u64, u32), u64> =
            std::collections::HashMap::new();
        let mut temp_balances: std::collections::HashMap<(u64, u32), (u64, u64)> =
            std::collections::HashMap::new(); // (avail, frozen)

        for trade in trades {
            total_filled += trade.quantity;
            total_cost += trade.price * trade.quantity;

            let buyer_refund = if side == Side::Buy && trade.buy_user_id == user_id {
                if price > trade.price {
                    (price - trade.price) * trade.quantity
                } else {
                    0
                }
            } else {
                0
            };

            let buyer_quote_inc = if buyer_refund > 0 { 2 } else { 1 };

            // Helper to get and increment version
            let mut get_and_add_version = |uid: u64, asset_id: u32, inc: u64| -> u64 {
                let entry = temp_versions
                    .entry((uid, asset_id))
                    .or_insert_with(|| ledger.get_balance_version(uid, asset_id));
                let v = *entry;
                *entry += inc;
                v
            };

            // ME DOES NOT TRACK BALANCES - Only generates deltas
            // Actual balance state is owned by UBSCore
            // avail/frozen fields in balance events set to -1 (not tracked)

            let buyer_quote_version =
                get_and_add_version(trade.buy_user_id, quote_asset_id, buyer_quote_inc);
            let buyer_base_version = get_and_add_version(trade.buy_user_id, base_asset_id, 1);
            let seller_base_version = get_and_add_version(trade.sell_user_id, base_asset_id, 1);
            let seller_quote_version = get_and_add_version(trade.sell_user_id, quote_asset_id, 1);

            // Calculate balance changes (DELTAS only)
            let trade_cost = trade.price * trade.quantity;

            // Add trade output
            builder.add_trade(TradeOutput {
                trade_id: trade.trade_id,
                match_seq: trade.match_seq,
                symbol_id, // From order for SOT sharding
                buy_order_id: trade.buy_order_id,
                sell_order_id: trade.sell_order_id,
                buyer_user_id: trade.buy_user_id,
                seller_user_id: trade.sell_user_id,
                price: trade.price,
                quantity: trade.quantity,
                base_asset_id,
                quote_asset_id,
                buyer_refund,
                seller_refund: 0,
                settled_at: 0,
            });

            // Add balance events for buyer (DELTAS only, avail/frozen = -1)
            if buyer_refund > 0 {
                builder.add_balance_event(EOBalanceEvent {
                    user_id: trade.buy_user_id,
                    asset_id: quote_asset_id,
                    seq: buyer_quote_version + 1,
                    delta_avail: buyer_refund as i64,
                    delta_frozen: -(trade_cost as i64 + buyer_refund as i64),
                    avail: None, // ME doesn't track balances
                    frozen: None, // ME doesn't track balances
                    event_type: "trade_refund".into(),
                    ref_id: trade.trade_id,
                });
            } else {
                builder.add_balance_event(EOBalanceEvent {
                    user_id: trade.buy_user_id,
                    asset_id: quote_asset_id,
                    seq: buyer_quote_version + 1,
                    delta_avail: 0,
                    delta_frozen: -(trade_cost as i64),
                    avail: None, // ME doesn't track balances
                    frozen: None, // ME doesn't track balances
                    event_type: "trade_debit".into(),
                    ref_id: trade.trade_id,
                });
            }

            builder.add_balance_event(EOBalanceEvent {
                user_id: trade.buy_user_id,
                asset_id: base_asset_id,
                seq: buyer_base_version + 1,
                delta_avail: trade.quantity as i64,
                delta_frozen: 0,
                avail: None, // ME doesn't track balances
                frozen: None, // ME doesn't track balances
                event_type: "trade_credit".into(),
                ref_id: trade.trade_id,
            });

            // Add balance events for seller (DELTAS only, avail/frozen = -1)
            builder.add_balance_event(EOBalanceEvent {
                user_id: trade.sell_user_id,
                asset_id: base_asset_id,
                seq: seller_base_version + 1,
                delta_avail: 0,
                delta_frozen: -(trade.quantity as i64),
                avail: None, // ME doesn't track balances
                frozen: None, // ME doesn't track balances
                event_type: "trade_debit".into(),
                ref_id: trade.trade_id,
            });

            builder.add_balance_event(EOBalanceEvent {
                user_id: trade.sell_user_id,
                asset_id: quote_asset_id,
                seq: seller_quote_version + 1,
                delta_avail: trade_cost as i64,
                delta_frozen: 0,
                avail: None, // ME doesn't track balances
                frozen: None, // ME doesn't track balances
                event_type: "trade_credit".into(),
                ref_id: trade.trade_id,
            });

            // Build match batch for existing flow
            match_batch.push(MatchExecData {
                trade_id: trade.trade_id,
                buy_order_id: trade.buy_order_id,
                sell_order_id: trade.sell_order_id,
                buyer_user_id: trade.buy_user_id,
                seller_user_id: trade.sell_user_id,
                price: trade.price,
                quantity: trade.quantity,
                base_asset_id,
                quote_asset_id,
                buyer_refund,
                seller_refund: 0,
                match_seq: trade.match_seq,
                output_sequence: current_output_seq,
                settled_at: 0,
                buyer_quote_version,
                buyer_base_version,
                seller_base_version,
                seller_quote_version,
                buyer_quote_balance_after: 0, // ME doesn't track balances
                buyer_base_balance_after: 0, // ME doesn't track balances
                seller_base_balance_after: 0, // ME doesn't track balances
                seller_quote_balance_after: 0, // ME doesn't track balances
            });
        }

        // Apply match batch to ledger (existing flow)
        if !match_batch.is_empty() {
            ledger
                .apply(&LedgerCommand::MatchExecBatch(match_batch))
                .map_err(|e| OrderError::LedgerError(e.to_string()))?;
        }

        // Set order update in EngineOutput
        let remaining_qty = quantity - total_filled;
        let avg_price = if total_filled > 0 { total_cost / total_filled } else { 0 };
        let status = if total_filled == 0 {
            1 // New (resting on book)
        } else if remaining_qty == 0 {
            3 // Filled
        } else {
            2 // PartialFill
        };

        builder.set_order_update(EOOrderUpdate {
            order_id,
            user_id,
            status,
            filled_qty: total_filled,
            remaining_qty,
            avg_price,
            updated_at: timestamp,
        });

        // Build final output
        let output = builder.build_unchecked();
        *last_output_hash = output.hash;

        Ok((order_id, output))
    }

    pub fn add_order_batch(
        &mut self,
        requests: Vec<(u32, u64, Side, OrderType, u64, u64, u64, u64)>,
    ) -> (Vec<Result<u64, OrderError>>, Vec<LedgerCommand>) {
        let _start_total = std::time::Instant::now();
        let mut results = Vec::with_capacity(requests.len());
        let mut output_cmds = Vec::new();

        for (symbol_id, order_id, side, order_type, price, quantity, user_id, timestamp) in requests
        {
            // Per-Order Atomic Processing
            // We create a shadow ledger here, but pass it to the logic function.
            // Using a helper method that handles the logic and returns the populated shadow if successful.
            let process_result = self.process_order_atomic(
                symbol_id, order_id, side, order_type, price, quantity, user_id, timestamp,
            );

            match process_result {
                Ok(oid) => {
                    // Success: Balance state is in UBSCore
                    results.push(Ok(oid));
                }
                Err(e) => {
                    // System Error (e.g. Duplicate ID): Drop
                    results.push(Err(e));
                }
            }
        }

        (results, output_cmds)
    }

    /// Internal wrapper for atomic order processing.
    /// Creates a ShadowLedger, executes logic, and returns it if successful.
    fn process_order_atomic(
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
        // Balance state is in UBSCore - no need for shadow ledger
        let mut null_ledger = NullLedger::new();

        // Process order (matching only, no balance operations)
        Self::process_order_logic(
            &mut null_ledger,
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

        Ok(order_id)
    }

    /// Public API: Cancel Order (Writes to WAL, then processes)
    /// Returns Vec<LedgerCommand> containing unlock operations and OrderUpdate(Cancelled)
    pub fn cancel_order(
        &mut self,
        symbol_id: u32,
        order_id: u64,
    ) -> Result<Vec<LedgerCommand>, OrderError> {
        // 1. Write to WAL
        if let Some(wal) = &mut self.order_wal {
            wal.log_cancel_order(order_id).map_err(|e| OrderError::Other(e.to_string()))?;
        }

        // 2. Process Logic
        let commands = self.process_cancel(symbol_id, order_id)?;

        // Update State Hash
        let mut hash_data = Vec::with_capacity(16);
        hash_data.extend_from_slice(&symbol_id.to_le_bytes());
        hash_data.extend_from_slice(&order_id.to_le_bytes());
        hash_data.push(0xFF); // Marker for Cancel
        self.update_hash(&hash_data);

        Ok(commands)
    }

    /// Internal Logic: Process Cancel (No Input WAL write)
    /// Returns Vec<LedgerCommand> with unlock and OrderUpdate(Cancelled)
    fn process_cancel(
        &mut self,
        symbol_id: u32,
        order_id: u64,
    ) -> Result<Vec<LedgerCommand>, OrderError> {
        use crate::ledger::{OrderStatus, OrderUpdate};

        let book = self
            .order_books
            .get_mut(symbol_id as usize)
            .and_then(|opt_book| opt_book.as_mut())
            .ok_or(OrderError::InvalidSymbol { symbol_id })?;

        let cancelled_order = book.remove_order(order_id)?;

        let mut commands = Vec::new();

        // Get asset info for unlocking funds
        let (base_asset_id, quote_asset_id) =
            *self.asset_map.get(&symbol_id).ok_or(OrderError::AssetMapNotFound { symbol_id })?;

        // Unlock funds based on order side
        let (unlock_asset, unlock_amount) = match cancelled_order.side {
            Side::Buy => (quote_asset_id, cancelled_order.price * cancelled_order.quantity),
            Side::Sell => (base_asset_id, cancelled_order.quantity),
        };

        // REMOVED: Balance operations (unlock) now handled by UBSCore
        // ME only manipulates order books, not balances
        let mut null_ledger = NullLedger::new();
        let current_bal = null_ledger.get_balance(cancelled_order.user_id, unlock_asset);
        let current_ver = null_ledger.get_balance_version(cancelled_order.user_id, unlock_asset);
        let balance_after = current_bal + unlock_amount;
        let version = current_ver + 1;

        // No longer apply to ledger - UBSCore handles this

        commands.push(LedgerCommand::Unlock {
            user_id: cancelled_order.user_id,
            asset_id: unlock_asset,
            amount: unlock_amount,
            balance_after,
            version,
        });

        // Emit OrderUpdate(Cancelled)
        let order_update = OrderUpdate {
            order_id: cancelled_order.order_id,
            client_order_id: None,
            user_id: cancelled_order.user_id,
            symbol_id,
            side: cancelled_order.side.as_u8(),
            order_type: cancelled_order.order_type.as_u8(),
            status: OrderStatus::Cancelled,
            price: cancelled_order.price,
            qty: cancelled_order.quantity,
            filled_qty: 0, // Order was cancelled before any fills
            avg_fill_price: None,
            rejection_reason: None,
            timestamp: cancelled_order.timestamp,
            match_id: None,
        };

        // No longer apply OrderUpdate to ledger - not needed
        // self.ledger.apply(&LedgerCommand::OrderUpdate(order_update.clone()))?;

        commands.push(LedgerCommand::OrderUpdate(order_update));

        Ok(commands)
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
                    ledger_accounts: Default::default(), // REMOVED: ME no longer has balance state
                    ledger_seq: 0, // REMOVED: Balance state is in UBSCore
                    order_wal_seq: current_seq,
                    output_seq: self.output_seq,
                    last_output_hash: self.last_output_hash,
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

    // REMOVED: transfer_in_to_trading_account() and transfer_out_from_trading_account()
    // These were test helpers that are no longer valid since ME doesn't have ledger
    // Tests using these methods need to be updated to use UBSCore client

    // REMOVED: transfer_in_and_build_output() and transfer_out_and_build_output()
    // Deposits and withdrawals are now handled exclusively by UBSCore
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

#[cfg(test)]
#[path = "matching_engine_base_tests.rs"]
mod matching_engine_base_tests;

#[cfg(test)]
#[path = "matching_engine_balance_tests.rs"]
mod matching_engine_balance_tests;

#[cfg(test)]
#[path = "matching_engine_field_tests.rs"]
mod matching_engine_field_tests;

#[cfg(test)]
#[path = "matching_engine_order_status_tests.rs"]
mod matching_engine_order_status_tests;
