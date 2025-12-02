use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "Buy"),
            Side::Sell => write!(f, "Sell"),
        }
    }
}

impl FromStr for Side {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Buy" => Ok(Side::Buy),
            "Sell" => Ok(Side::Sell),
            _ => Err(format!("Unknown side: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::Limit => write!(f, "Limit"),
            OrderType::Market => write!(f, "Market"),
        }
    }
}

impl FromStr for OrderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Limit" => Ok(OrderType::Limit),
            "Market" => Ok(OrderType::Market),
            _ => Err(format!("Unknown order type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    Accepted,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Order {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: Side,
    pub order_type: OrderType,
    pub price: u64,
    pub quantity: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Trade {
    pub trade_id: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub buy_user_id: u64,
    pub sell_user_id: u64,
    pub price: u64,
    pub quantity: u64,
    pub match_id: u64,
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
        symbol_id: u32,
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
        symbol_id: u32,
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
