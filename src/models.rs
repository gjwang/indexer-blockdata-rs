use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BalanceUpdate {
    pub asset: String,
    pub available: f64,
    pub locked: f64,
    pub total: f64,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderUpdate {
    pub order_id: String,
    pub symbol: String,
    pub side: String,        // "buy" or "sell"
    pub order_type: String,  // "limit", "market", etc.
    pub status: String,      // "new", "filled", "cancelled", etc.
    pub price: f64,
    pub quantity: f64,
    pub filled_quantity: f64,
    pub remaining_quantity: f64,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PositionUpdate {
    pub symbol: String,
    pub side: String,           // "long" or "short"
    pub quantity: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub liquidation_price: f64,
    pub unrealized_pnl: f64,
    pub leverage: f64,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum UserUpdate {
    Balance(BalanceUpdate),
    Order(OrderUpdate),
    Position(PositionUpdate),
}
