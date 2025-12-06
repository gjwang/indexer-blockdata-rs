//! Engine Output - Atomic chain-hashed output bundle from Matching Engine
//!
//! Each EngineOutput contains:
//! - The original input (with CRC)
//! - All output effects (order updates, trades, balance events)
//! - Chain hash for verification (prev_hash → hash linkage)

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Complete output bundle from Matching Engine
/// One input → One EngineOutput (with all effects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineOutput {
    // === CHAIN HEADER ===
    /// Monotonically increasing output sequence
    pub output_seq: u64,

    /// Hash of previous EngineOutput (chain linkage)
    /// Genesis output has prev_hash = [0u8; 32]
    #[serde(with = "hash_serde")]
    pub prev_hash: [u8; 32],

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

    // === SELF HASH ===
    /// SHA256 of (output_seq || prev_hash || input || outputs)
    #[serde(with = "hash_serde")]
    pub hash: [u8; 32],
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
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
    pub side: u8,        // 1=Buy, 2=Sell
    pub order_type: u8,  // 1=Limit, 2=Market
    pub price: u64,
    pub quantity: u64,
    pub cid: String,     // Client order ID
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

/// Balance event (unified format for all balance changes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEvent {
    pub user_id: u64,
    pub asset_id: u32,
    pub seq: u64,          // Balance sequence (version)
    pub delta_avail: i64,  // Change to available
    pub delta_frozen: i64, // Change to frozen
    pub avail: i64,        // Balance after
    pub frozen: i64,       // Frozen after
    pub event_type: String, // "deposit", "lock", "unlock", "withdraw", "trade_credit", "trade_debit"
    pub ref_id: u64,       // Order ID or Trade ID
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

impl EngineOutput {
    /// Create a new EngineOutput with computed hash
    pub fn new(
        output_seq: u64,
        prev_hash: [u8; 32],
        input: InputBundle,
        order_update: Option<OrderUpdate>,
        trades: Vec<TradeOutput>,
        balance_events: Vec<BalanceEvent>,
    ) -> Self {
        let mut output = Self {
            output_seq,
            prev_hash,
            input,
            order_update,
            trades,
            balance_events,
            hash: [0u8; 32],
        };
        output.hash = output.compute_hash();
        output
    }

    /// Compute SHA256 hash of this output
    pub fn compute_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Chain header
        hasher.update(self.output_seq.to_le_bytes());
        hasher.update(&self.prev_hash);

        // Input
        hasher.update(self.input.input_seq.to_le_bytes());
        hasher.update(self.input.crc.to_le_bytes());
        self.hash_input_data(&mut hasher);

        // Order update
        if let Some(ref ou) = self.order_update {
            hasher.update([1u8]); // marker: has order update
            hasher.update(ou.order_id.to_le_bytes());
            hasher.update(ou.user_id.to_le_bytes());
            hasher.update([ou.status]);
            hasher.update(ou.filled_qty.to_le_bytes());
            hasher.update(ou.remaining_qty.to_le_bytes());
            hasher.update(ou.avg_price.to_le_bytes());
            hasher.update(ou.updated_at.to_le_bytes());
        } else {
            hasher.update([0u8]); // marker: no order update
        }

        // Trades
        hasher.update((self.trades.len() as u32).to_le_bytes());
        for trade in &self.trades {
            hasher.update(trade.trade_id.to_le_bytes());
            hasher.update(trade.match_seq.to_le_bytes());
            hasher.update(trade.buy_order_id.to_le_bytes());
            hasher.update(trade.sell_order_id.to_le_bytes());
            hasher.update(trade.buyer_user_id.to_le_bytes());
            hasher.update(trade.seller_user_id.to_le_bytes());
            hasher.update(trade.price.to_le_bytes());
            hasher.update(trade.quantity.to_le_bytes());
            hasher.update(trade.base_asset_id.to_le_bytes());
            hasher.update(trade.quote_asset_id.to_le_bytes());
            hasher.update(trade.buyer_refund.to_le_bytes());
            hasher.update(trade.seller_refund.to_le_bytes());
            hasher.update(trade.settled_at.to_le_bytes());
        }

        // Balance events
        hasher.update((self.balance_events.len() as u32).to_le_bytes());
        for event in &self.balance_events {
            hasher.update(event.user_id.to_le_bytes());
            hasher.update(event.asset_id.to_le_bytes());
            hasher.update(event.seq.to_le_bytes());
            hasher.update(event.delta_avail.to_le_bytes());
            hasher.update(event.delta_frozen.to_le_bytes());
            hasher.update(event.avail.to_le_bytes());
            hasher.update(event.frozen.to_le_bytes());
            hasher.update(event.event_type.as_bytes());
            hasher.update(event.ref_id.to_le_bytes());
        }

        hasher.finalize().into()
    }

    fn hash_input_data(&self, hasher: &mut Sha256) {
        match &self.input.data {
            InputData::PlaceOrder(o) => {
                hasher.update([1u8]); // type marker
                hasher.update(o.order_id.to_le_bytes());
                hasher.update(o.user_id.to_le_bytes());
                hasher.update(o.symbol_id.to_le_bytes());
                hasher.update([o.side]);
                hasher.update([o.order_type]);
                hasher.update(o.price.to_le_bytes());
                hasher.update(o.quantity.to_le_bytes());
                hasher.update(o.cid.as_bytes());
                hasher.update(o.created_at.to_le_bytes());
            }
            InputData::CancelOrder(c) => {
                hasher.update([2u8]);
                hasher.update(c.order_id.to_le_bytes());
                hasher.update(c.user_id.to_le_bytes());
                hasher.update(c.created_at.to_le_bytes());
            }
            InputData::Deposit(d) => {
                hasher.update([3u8]);
                hasher.update(d.user_id.to_le_bytes());
                hasher.update(d.asset_id.to_le_bytes());
                hasher.update(d.amount.to_le_bytes());
                hasher.update(d.created_at.to_le_bytes());
            }
            InputData::Withdraw(w) => {
                hasher.update([4u8]);
                hasher.update(w.user_id.to_le_bytes());
                hasher.update(w.asset_id.to_le_bytes());
                hasher.update(w.amount.to_le_bytes());
                hasher.update(w.created_at.to_le_bytes());
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
    pub fn verify_prev_hash(&self, expected_prev_hash: &[u8; 32]) -> bool {
        self.prev_hash == *expected_prev_hash
    }
}

impl InputBundle {
    /// Create input bundle with CRC computed from serialized data
    pub fn new(input_seq: u64, data: InputData) -> Self {
        let serialized = serde_json::to_vec(&data).unwrap_or_default();
        let crc = crc32fast::hash(&serialized);
        Self {
            input_seq,
            crc,
            data,
        }
    }

    /// Verify CRC against stored value
    pub fn verify_crc(&self) -> bool {
        let serialized = serde_json::to_vec(&self.data).unwrap_or_default();
        let computed_crc = crc32fast::hash(&serialized);
        self.crc == computed_crc
    }
}

/// Serde helper for [u8; 32] hash arrays
mod hash_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hash: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(hash))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("hash must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

/// Genesis hash (all zeros) for first output
pub const GENESIS_HASH: [u8; 32] = [0u8; 32];

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
                avail: 8500000,
                frozen: 1500000,
                event_type: "lock".into(),
                ref_id: 12345,
            }],
        );

        // Verify hash is non-zero
        assert_ne!(output.hash, [0u8; 32]);

        // Verify self-verification
        assert!(output.verify());

        // Verify prev_hash check
        assert!(output.verify_prev_hash(&GENESIS_HASH));
    }

    #[test]
    fn test_chain_verification() {
        let input1 = InputBundle::new(1, InputData::Deposit(DepositInput {
            user_id: 1001,
            asset_id: 1,
            amount: 1000000,
            created_at: 1700000000000,
        }));

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
                avail: 1000000,
                frozen: 0,
                event_type: "deposit".into(),
                ref_id: 0,
            }],
        );

        let input2 = InputBundle::new(2, InputData::Deposit(DepositInput {
            user_id: 1001,
            asset_id: 2,
            amount: 2000000,
            created_at: 1700000001000,
        }));

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
                avail: 2000000,
                frozen: 0,
                event_type: "deposit".into(),
                ref_id: 0,
            }],
        );

        // Verify chain
        assert!(output2.verify_chain(&output1));
        assert!(output2.verify_prev_hash(&output1.hash));

        // Verify integrity
        assert!(output1.verify());
        assert!(output2.verify());
    }

    #[test]
    fn test_input_crc() {
        let input = InputBundle::new(1, InputData::Deposit(DepositInput {
            user_id: 1001,
            asset_id: 1,
            amount: 1000000,
            created_at: 1700000000000,
        }));

        assert!(input.verify_crc());
    }
}
