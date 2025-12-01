use serde::{Deserialize, Serialize};

mod float_as_string {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BalanceUpdate {
    pub asset: String,
    #[serde(with = "float_as_string")]
    pub available: f64,
    #[serde(with = "float_as_string")]
    pub locked: f64,
    #[serde(with = "float_as_string")]
    pub total: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderUpdate {
    pub order_id: String,
    pub symbol: u32,
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
    pub symbol: u32,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum OrderRequest {
    PlaceOrder {
        order_id: u64,
        user_id: u64,
        symbol: u32,
        side: String, // "Buy" or "Sell"
        price: u64,
        quantity: u64,
        order_type: String, // "Limit"
    },
    CancelOrder {
        order_id: u64,
        user_id: u64,
        symbol: u32,
    }
}

// --- Engine Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol: u32,
    pub side: Side,
    pub order_type: OrderType,
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

#[derive(Debug, Clone)]
pub enum OrderError {
    InsufficientFunds {
        user_id: u64,
        asset_id: u32,
        required: u64,
        available: u64,
    },
    InvalidSymbol {
        symbol_id: usize,
    },
    SymbolMismatch {
        expected: u32,
        actual: u32,
    },
    DuplicateOrderId {
        order_id: u64,
    },
    OrderNotFound {
        order_id: u64,
    },
    AssetMapNotFound {
        symbol_id: usize,
    },
    LedgerError(String),
    Other(String),
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderError::InsufficientFunds {
                user_id,
                asset_id,
                required,
                available,
            } => write!(
                f,
                "Insufficient funds: User {} needs {} of Asset {}, has {}",
                user_id, required, asset_id, available
            ),
            OrderError::InvalidSymbol { symbol_id } => {
                write!(f, "Invalid symbol ID: {}", symbol_id)
            }
            OrderError::SymbolMismatch { expected, actual } => write!(
                f,
                "Symbol mismatch: expected '{}', got '{}'",
                expected, actual
            ),
            OrderError::DuplicateOrderId { order_id } => {
                write!(f, "Duplicate Order ID: {}", order_id)
            }
            OrderError::OrderNotFound { order_id } => write!(f, "Order Not Found: {}", order_id),
            OrderError::AssetMapNotFound { symbol_id } => {
                write!(f, "Asset map not found for symbol ID: {}", symbol_id)
            }
            OrderError::LedgerError(msg) => write!(f, "Ledger Error: {}", msg),
            OrderError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl std::error::Error for OrderError {}

impl From<String> for OrderError {
    fn from(err: String) -> Self {
        OrderError::Other(err)
    }
}
