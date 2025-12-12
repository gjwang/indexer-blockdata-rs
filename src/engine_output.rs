//! Engine Output - Atomic chain-hashed output bundle from Matching Engine
//!
//! Each EngineOutput contains:
//! - The original input (with CRC)
//! - All output effects (order updates, trades, balance events)
//! - Chain hash for verification (prev_hash → hash linkage)
//! Uses xxh3_64 for fast 64-bit hashing (consistent with PURE_MEMORY_SMR_ARCH.md)

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

/// Complete output bundle from Matching Engine
/// One input → One EngineOutput (with all effects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineOutput {
    // === CHAIN HEADER ===
    /// Monotonically increasing output sequence
    pub output_seq: u64,

    /// Hash of previous EngineOutput (chain linkage)
    /// Genesis output has prev_hash = 0
    pub prev_hash: u64,

    /// Primary symbol for this output (for SOT sharding)
    /// 0 = system/multi-symbol operations, otherwise symbol_id from first trade/order
    pub symbol_id: u32,

    // === INPUT (Self-contained) ===
    /// Original input data with integrity check
    pub input: InputBundle,

    // === OUTPUTS ===
    /// Order status update (optional - not all inputs produce order updates)
    pub order_update: Option<OrderUpdate>,

    /// Trades generated (0, 1, or many)
    pub trades: Vec<TradeOutput>,

    /// Balance changes (all in one place)
    pub balance_events: Vec<BalanceEvent>,

    // === ORDER LIFECYCLE EVENTS (for active_orders table) ===
    /// New orders placed (for tracking in active_orders)
    pub order_placements: Vec<OrderPlacement>,

    /// Order completions (filled or cancelled)
    pub order_completions: Vec<OrderCompletion>,

    // === SELF HASH ===
    /// xxh3_64 of (output_seq || prev_hash || input || outputs)
    pub hash: u64,
}

/// Original input with integrity check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputBundle {
    /// Input sequence (idempotency key)
    pub input_seq: u64,

    /// CRC32 of the original input data bytes
    pub crc: u32,

    /// Original input data
    pub data: InputData,
}

/// All possible input types
/// NOTE: No serde tags - bincode uses enum discriminant (more efficient)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputData {
    PlaceOrder(PlaceOrderInput),
    CancelOrder(CancelOrderInput),
    Deposit(DepositInput),
    Withdraw(WithdrawInput),
}

/// Place order input (full original data for order history)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceOrderInput {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,       // 1=Buy, 2=Sell
    pub order_type: u8, // 1=Limit, 2=Market
    pub price: u64,
    pub quantity: u64,
    pub cid: String, // Client order ID
    pub created_at: u64,
}

/// Cancel order input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelOrderInput {
    pub order_id: u64,
    pub user_id: u64,
    pub created_at: u64,
}

/// Deposit input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositInput {
    pub user_id: u64,
    pub asset_id: u32,
    pub amount: u64,
    pub created_at: u64,
}

/// Withdraw input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawInput {
    pub user_id: u64,
    pub asset_id: u32,
    pub amount: u64,
    pub created_at: u64,
}

/// Trade output (derived from trade execution)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeOutput {
    pub trade_id: u64,
    pub match_seq: u64,
    pub symbol_id: u32, // Symbol for this trade (for SOT sharding)
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub buyer_user_id: u64,
    pub seller_user_id: u64,
    pub price: u64,
    pub quantity: u64,
    pub base_asset_id: u32,
    pub quote_asset_id: u32,
    pub buyer_refund: u64,
    pub seller_refund: u64,
    pub settled_at: u64,
}

/// Balance event (ledger entry)
///
/// Design Notes:
/// - delta_avail/delta_frozen are i64 because they're SIGNED changes (can be negative)
/// - avail/frozen are Option<u64> because balances are UNSIGNED (never negative)
/// - None means "not tracked" (ME doesn't track balances, only generates deltas)
/// - Settlement/UBSCore reconstruct actual balances from deltas
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEvent {
    pub user_id: u64,
    pub asset_id: u32,
    pub seq: u64,              // Balance sequence (version)
    pub delta_avail: i64,      // Change to available (SIGNED: can be negative for debits)
    pub delta_frozen: i64,     // Change to frozen (SIGNED: can be negative for unlocks)
    pub avail: Option<u64>,    // Balance after (None = not tracked by ME)
    pub frozen: Option<u64>,   // Frozen after (None = not tracked by ME)
    pub event_type: String,    // "deposit", "lock", "unlock", "withdraw", "trade_credit", "trade_debit"
    pub ref_id: u64,           // Order ID or Trade ID
}

impl BalanceEvent {
    /// Get available balance (enforced API)
    #[inline(always)]
    pub fn avail(&self) -> Option<u64> {
        self.avail
    }

    /// Get frozen balance (enforced API)
    #[inline(always)]
    pub fn frozen(&self) -> Option<u64> {
        self.frozen
    }
}

/// Order status update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub order_id: u64,
    pub user_id: u64,
    pub status: u8, // 1=Accepted, 2=PartialFill, 3=Filled, 4=Cancelled, 5=Rejected
    pub filled_qty: u64,
    pub remaining_qty: u64,
    pub avg_price: u64,
    pub updated_at: u64,
}

/// Order placement (for active_orders tracking)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderPlacement {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,          // 0=Buy, 1=Sell
    pub order_type: u8,    // 0=Market, 1=Limit
    pub price: u64,
    pub quantity: u64,
    pub created_at: u64,
}

/// Order completion (filled or cancelled)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderCompletion {
    pub order_id: u64,
    pub reason: CompletionReason,
    pub completed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionReason {
    Filled,
    Cancelled,
}

impl EngineOutput {
    /// Create a new EngineOutput with computed hash
    pub fn new(
        output_seq: u64,
        prev_hash: u64,
        input: InputBundle,
        order_update: Option<OrderUpdate>,
        trades: Vec<TradeOutput>,
        balance_events: Vec<BalanceEvent>,
    ) -> Self {
        // Determine symbol_id from first trade if available
        // 0 = system operations (deposits, withdrawals, no trades)
        let symbol_id = trades.first().map(|t| t.symbol_id).unwrap_or(0);

        let mut output = Self {
            output_seq,
            prev_hash,
            symbol_id,
            input,
            order_update,
            trades,
            balance_events,
            order_placements: Vec::new(),
            order_completions: Vec::new(),
            hash: 0,
        };
        output.hash = output.compute_hash();
        output
    }

    /// Compute xxh3_64 hash of this output
    pub fn compute_hash(&self) -> u64 {
        let mut data = Vec::new();

        // Chain header
        data.extend_from_slice(&self.output_seq.to_le_bytes());
        data.extend_from_slice(&self.prev_hash.to_le_bytes());
        data.extend_from_slice(&self.symbol_id.to_le_bytes()); // Include symbol_id in hash

        // Input
        data.extend_from_slice(&self.input.input_seq.to_le_bytes());
        data.extend_from_slice(&self.input.crc.to_le_bytes());
        self.hash_input_data(&mut data);

        // Order update
        if let Some(ref ou) = self.order_update {
            data.push(1u8); // marker: has order update
            data.extend_from_slice(&ou.order_id.to_le_bytes());
            data.extend_from_slice(&ou.user_id.to_le_bytes());
            data.push(ou.status);
            data.extend_from_slice(&ou.filled_qty.to_le_bytes());
            data.extend_from_slice(&ou.remaining_qty.to_le_bytes());
            data.extend_from_slice(&ou.avg_price.to_le_bytes());
            data.extend_from_slice(&ou.updated_at.to_le_bytes());
        } else {
            data.push(0u8); // marker: no order update
        }

        // Trades
        data.extend_from_slice(&(self.trades.len() as u32).to_le_bytes());
        for trade in &self.trades {
            data.extend_from_slice(&trade.trade_id.to_le_bytes());
            data.extend_from_slice(&trade.match_seq.to_le_bytes());
            data.extend_from_slice(&trade.buy_order_id.to_le_bytes());
            data.extend_from_slice(&trade.sell_order_id.to_le_bytes());
            data.extend_from_slice(&trade.buyer_user_id.to_le_bytes());
            data.extend_from_slice(&trade.seller_user_id.to_le_bytes());
            data.extend_from_slice(&trade.price.to_le_bytes());
            data.extend_from_slice(&trade.quantity.to_le_bytes());
            data.extend_from_slice(&trade.base_asset_id.to_le_bytes());
            data.extend_from_slice(&trade.quote_asset_id.to_le_bytes());
            data.extend_from_slice(&trade.buyer_refund.to_le_bytes());
            data.extend_from_slice(&trade.seller_refund.to_le_bytes());
            data.extend_from_slice(&trade.settled_at.to_le_bytes());
        }

        // Balance events
        data.extend_from_slice(&(self.balance_events.len() as u32).to_le_bytes());
        for event in &self.balance_events {
            data.extend_from_slice(&event.user_id.to_le_bytes());
            data.extend_from_slice(&event.asset_id.to_le_bytes());
            data.extend_from_slice(&event.seq.to_le_bytes());
            data.extend_from_slice(&event.delta_avail.to_le_bytes());
            data.extend_from_slice(&event.delta_frozen.to_le_bytes());
            data.extend_from_slice(&event.avail().unwrap_or(0).to_le_bytes());
            data.extend_from_slice(&event.frozen().unwrap_or(0).to_le_bytes());
            data.extend_from_slice(event.event_type.as_bytes());
            data.extend_from_slice(&event.ref_id.to_le_bytes());
        }

        xxh3_64(&data)
    }

    fn hash_input_data(&self, data: &mut Vec<u8>) {
        match &self.input.data {
            InputData::PlaceOrder(o) => {
                data.push(1u8); // type marker
                data.extend_from_slice(&o.order_id.to_le_bytes());
                data.extend_from_slice(&o.user_id.to_le_bytes());
                data.extend_from_slice(&o.symbol_id.to_le_bytes());
                data.push(o.side);
                data.push(o.order_type);
                data.extend_from_slice(&o.price.to_le_bytes());
                data.extend_from_slice(&o.quantity.to_le_bytes());
                data.extend_from_slice(o.cid.as_bytes());
                data.extend_from_slice(&o.created_at.to_le_bytes());
            }
            InputData::CancelOrder(c) => {
                data.push(2u8);
                data.extend_from_slice(&c.order_id.to_le_bytes());
                data.extend_from_slice(&c.user_id.to_le_bytes());
                data.extend_from_slice(&c.created_at.to_le_bytes());
            }
            InputData::Deposit(d) => {
                data.push(3u8);
                data.extend_from_slice(&d.user_id.to_le_bytes());
                data.extend_from_slice(&d.asset_id.to_le_bytes());
                data.extend_from_slice(&d.amount.to_le_bytes());
                data.extend_from_slice(&d.created_at.to_le_bytes());
            }
            InputData::Withdraw(w) => {
                data.push(4u8);
                data.extend_from_slice(&w.user_id.to_le_bytes());
                data.extend_from_slice(&w.asset_id.to_le_bytes());
                data.extend_from_slice(&w.amount.to_le_bytes());
                data.extend_from_slice(&w.created_at.to_le_bytes());
            }
        }
    }

    /// Verify this output's integrity (hash matches computed hash)
    pub fn verify(&self) -> bool {
        self.hash == self.compute_hash()
    }

    /// Verify chain linkage with previous output
    pub fn verify_chain(&self, prev: &EngineOutput) -> bool {
        self.output_seq == prev.output_seq + 1 && self.prev_hash == prev.hash
    }

    /// Verify chain linkage with just the previous hash
    pub fn verify_prev_hash(&self, expected_prev_hash: u64) -> bool {
        self.prev_hash == expected_prev_hash
    }
}

impl InputBundle {
    /// Create input bundle with CRC computed from serialized data
    pub fn new(input_seq: u64, data: InputData) -> Self {
        let serialized = serde_json::to_vec(&data).unwrap_or_default();
        let crc = crc32fast::hash(&serialized);
        Self { input_seq, crc, data }
    }

    /// Verify CRC against stored value
    pub fn verify_crc(&self) -> bool {
        let serialized = serde_json::to_vec(&self.data).unwrap_or_default();
        let computed_crc = crc32fast::hash(&serialized);
        self.crc == computed_crc
    }
}

/// Genesis hash (zero) for first output
pub const GENESIS_HASH: u64 = 0;

/// Builder for incrementally constructing EngineOutput during order processing
#[derive(Debug)]
pub struct EngineOutputBuilder {
    output_seq: u64,
    prev_hash: u64,
    input: Option<InputBundle>,
    order_update: Option<OrderUpdate>,
    trades: Vec<TradeOutput>,
    balance_events: Vec<BalanceEvent>,
    order_placements: Vec<OrderPlacement>,
    order_completions: Vec<OrderCompletion>,
}

impl EngineOutputBuilder {
    /// Create a new builder with chain header
    pub fn new(output_seq: u64, prev_hash: u64) -> Self {
        Self {
            output_seq,
            prev_hash,
            input: None,
            order_update: None,
            trades: Vec::new(),
            balance_events: Vec::new(),
            order_placements: Vec::new(),
            order_completions: Vec::new(),
        }
    }

    /// Set the input bundle
    pub fn with_input(mut self, input: InputBundle) -> Self {
        self.input = Some(input);
        self
    }

    /// Set the input bundle (mutable reference version)
    pub fn set_input(&mut self, input: InputBundle) {
        self.input = Some(input);
    }

    /// Set the order update
    pub fn with_order_update(mut self, update: OrderUpdate) -> Self {
        self.order_update = Some(update);
        self
    }

    /// Set the order update (mutable reference version)
    pub fn set_order_update(&mut self, update: OrderUpdate) {
        self.order_update = Some(update);
    }

    /// Add a trade output
    pub fn add_trade(&mut self, trade: TradeOutput) {
        self.trades.push(trade);
    }

    /// Add a balance event
    pub fn add_balance_event(&mut self, event: BalanceEvent) {
        self.balance_events.push(event);
    }

    /// Add an order placement event
    pub fn add_order_placement(&mut self, placement: OrderPlacement) {
        self.order_placements.push(placement);
    }

    /// Add an order completion event
    pub fn add_order_completion(&mut self, completion: OrderCompletion) {
        self.order_completions.push(completion);
    }

    /// Build the final EngineOutput (computes hash)
    /// Returns None if input is not set
    pub fn build(self) -> Option<EngineOutput> {
        let input = self.input?;
        let symbol_id = self.trades.first().map(|t| t.symbol_id).unwrap_or(0);

        let mut output = EngineOutput {
            output_seq: self.output_seq,
            prev_hash: self.prev_hash,
            symbol_id,
            input,
            order_update: self.order_update,
            trades: self.trades,
            balance_events: self.balance_events,
            order_placements: self.order_placements,
            order_completions: self.order_completions,
            hash: 0,
        };
        output.hash = output.compute_hash();
        Some(output)
    }

    /// Build the final EngineOutput, panicking if input is not set
    pub fn build_unchecked(self) -> EngineOutput {
        self.build().expect("EngineOutputBuilder: input must be set before build")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_output_hash() {
        let input = InputBundle::new(
            1,
            InputData::PlaceOrder(PlaceOrderInput {
                order_id: 12345,
                user_id: 1001,
                symbol_id: 0,
                side: 1,
                order_type: 1,
                price: 15000,
                quantity: 100,
                cid: "test_order".into(),
                created_at: 1700000000000,
            }),
        );

        let output = EngineOutput::new(
            1,
            GENESIS_HASH,
            input,
            Some(OrderUpdate {
                order_id: 12345,
                user_id: 1001,
                status: 1,
                filled_qty: 0,
                remaining_qty: 100,
                avg_price: 0,
                updated_at: 1700000000000,
            }),
            vec![],
            vec![BalanceEvent {
                user_id: 1001,
                asset_id: 2,
                seq: 1,
                delta_avail: -1500000,
                delta_frozen: 1500000,
                avail: Some(8500000),
                frozen: Some(1500000),
                event_type: "lock".into(),
                ref_id: 12345,
            }],
        );

        // Verify hash is non-zero
        assert_ne!(output.hash, 0);

        // Verify self-verification
        assert!(output.verify());

        // Verify prev_hash check
        assert!(output.verify_prev_hash(GENESIS_HASH));
    }

    #[test]
    fn test_chain_verification() {
        let input1 = InputBundle::new(
            1,
            InputData::Deposit(DepositInput {
                user_id: 1001,
                asset_id: 1,
                amount: 1000000,
                created_at: 1700000000000,
            }),
        );

        let output1 = EngineOutput::new(
            1,
            GENESIS_HASH,
            input1,
            None,
            vec![],
            vec![BalanceEvent {
                user_id: 1001,
                asset_id: 1,
                seq: 1,
                delta_avail: 1000000,
                delta_frozen: 0,
                avail: Some(1000000),
                frozen: Some(0),
                event_type: "deposit".into(),
                ref_id: 0,
            }],
        );

        let input2 = InputBundle::new(
            2,
            InputData::Deposit(DepositInput {
                user_id: 1001,
                asset_id: 2,
                amount: 2000000,
                created_at: 1700000001000,
            }),
        );

        let output2 = EngineOutput::new(
            2,
            output1.hash, // Chain to previous
            input2,
            None,
            vec![],
            vec![BalanceEvent {
                user_id: 1001,
                asset_id: 2,
                seq: 1,
                delta_avail: 2000000,
                delta_frozen: 0,
                avail: Some(2000000),
                frozen: Some(0),
                event_type: "deposit".into(),
                ref_id: 0,
            }],
        );

        // Verify chain
        assert!(output2.verify_chain(&output1));
        assert!(output2.verify_prev_hash(output1.hash));

        // Verify integrity
        assert!(output1.verify());
        assert!(output2.verify());
    }

    #[test]
    fn test_input_crc() {
        let input = InputBundle::new(
            1,
            InputData::Deposit(DepositInput {
                user_id: 1001,
                asset_id: 1,
                amount: 1000000,
                created_at: 1700000000000,
            }),
        );

        assert!(input.verify_crc());
    }

    #[test]
    fn test_engine_output_builder() {
        // Test the builder pattern
        let mut builder = EngineOutputBuilder::new(1, GENESIS_HASH);

        // Set input
        builder.set_input(InputBundle::new(
            100,
            InputData::PlaceOrder(PlaceOrderInput {
                order_id: 12345,
                user_id: 1001,
                symbol_id: 0,
                side: 1,       // Buy
                order_type: 1, // Limit
                price: 15000,
                quantity: 100,
                cid: "test_cid".into(),
                created_at: 1700000000000,
            }),
        ));

        // Add order update
        builder.set_order_update(OrderUpdate {
            order_id: 12345,
            user_id: 1001,
            status: 1,
            filled_qty: 100,
            remaining_qty: 0,
            avg_price: 14950,
            updated_at: 1700000000001,
        });

        // Add a trade
        builder.add_trade(TradeOutput {
            trade_id: 99999,
            match_seq: 1,
            symbol_id: 1, // BTC/USDT
            buy_order_id: 12345,
            sell_order_id: 12346,
            buyer_user_id: 1001,
            seller_user_id: 1002,
            price: 14950,
            quantity: 100,
            base_asset_id: 1,
            quote_asset_id: 2,
            buyer_refund: 5000, // 15000*100 - 14950*100 = 5000
            seller_refund: 0,
            settled_at: 0,
        });

        // Add balance events
        builder.add_balance_event(BalanceEvent {
            user_id: 1001,
            asset_id: 2, // quote deducted
            seq: 1,
            delta_avail: -1495000,
            delta_frozen: 0,
            avail: Some(505000),
            frozen: Some(0),
            event_type: "trade_debit".into(),
            ref_id: 99999,
        });

        builder.add_balance_event(BalanceEvent {
            user_id: 1001,
            asset_id: 1, // base credited
            seq: 1,
            delta_avail: 100,
            delta_frozen: 0,
            avail: Some(100),
            frozen: Some(0),
            event_type: "trade_credit".into(),
            ref_id: 99999,
        });

        // Build
        let output = builder.build().expect("should build successfully");

        // Verify
        assert_eq!(output.output_seq, 1);
        assert_eq!(output.prev_hash, GENESIS_HASH);
        assert!(output.verify());
        assert!(output.order_update.is_some());
        assert_eq!(output.trades.len(), 1);
        assert_eq!(output.balance_events.len(), 2);
    }

    #[test]
    fn test_engine_output_builder_no_input() {
        // Builder should return None if input is not set
        let builder = EngineOutputBuilder::new(1, GENESIS_HASH);
        assert!(builder.build().is_none());
    }
}
