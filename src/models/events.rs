use super::serde_utils as float_as_string;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BalanceUpdate {
    pub asset: String,
    #[serde(with = "float_as_string")]
    pub avail: f64,
    #[serde(with = "float_as_string")]
    pub locked: f64,
    #[serde(with = "float_as_string")]
    pub total: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderUpdate {
    pub order_id: String,
    pub symbol_id: u32,
    pub side: String,       // "buy" or "sell"
    pub order_type: String, // "limit", "market", etc.
    pub status: String,     // "new", "filled", "cancelled", etc.
    #[serde(with = "float_as_string")]
    pub price: f64,
    #[serde(with = "float_as_string")]
    pub quantity: f64,
    #[serde(with = "float_as_string")]
    pub filled_quantity: f64,
    #[serde(with = "float_as_string")]
    pub remaining_quantity: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PositionUpdate {
    pub symbol_id: u32,
    pub side: String, // "long" or "short"
    #[serde(with = "float_as_string")]
    pub quantity: f64,
    #[serde(with = "float_as_string")]
    pub entry_price: f64,
    #[serde(with = "float_as_string")]
    pub mark_price: f64,
    #[serde(with = "float_as_string")]
    pub liquidation_price: f64,
    #[serde(with = "float_as_string")]
    pub unrealized_pnl: f64,
    #[serde(with = "float_as_string")]
    pub leverage: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum UserUpdate {
    Balance(BalanceUpdate),
    Order(OrderUpdate),
    Position(PositionUpdate),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamMessage {
    #[serde(flatten)]
    pub update: UserUpdate,
    pub ts_ms: i64,
}
