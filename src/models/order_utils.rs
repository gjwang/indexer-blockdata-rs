use rust_decimal::Decimal;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Side {
    Buy = 1,
    Sell = 2,
}

impl Side {
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Side::Buy),
            2 => Some(Side::Sell),
            _ => None,
        }
    }
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
#[repr(u8)]
pub enum OrderType {
    Limit = 1,
    Market = 2,
}

impl OrderType {
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(OrderType::Limit),
            2 => Some(OrderType::Market),
            _ => None,
        }
    }
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

impl OrderStatus {
    pub fn code(&self) -> u8 {
        match self {
            OrderStatus::New => 1,
            OrderStatus::Accepted => 2,
            OrderStatus::PartiallyFilled => 3,
            OrderStatus::Filled => 4,
            OrderStatus::Cancelled => 5,
            OrderStatus::Rejected(_) => 6,
        }
    }

    pub fn from_code(code: u8, rejection_reason: Option<String>) -> Self {
        match code {
            1 => OrderStatus::New,
            2 => OrderStatus::Accepted,
            3 => OrderStatus::PartiallyFilled,
            4 => OrderStatus::Filled,
            5 => OrderStatus::Cancelled,
            6 => OrderStatus::Rejected(rejection_reason.unwrap_or_default()),
            _ => OrderStatus::New, // Default or Error
        }
    }
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
    pub match_seq: u64,
}

#[derive(Debug, Clone)]
pub enum OrderError {
    InsufficientFunds { user_id: u64, asset_id: u32, required: u64, available: u64 },
    InvalidSymbol { symbol_id: u32 },
    SymbolMismatch { expected: u32, actual: u32 },
    DuplicateOrderId { order_id: u64 },
    OrderNotFound { order_id: u64 },
    AssetMapNotFound { symbol_id: u32 },
    LedgerError(String),
    Other(String),
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderError::InsufficientFunds { user_id, asset_id, required, available } => write!(
                f,
                "Insufficient funds: User {} needs {} of Asset {}, has {}",
                user_id, required, asset_id, available
            ),
            OrderError::InvalidSymbol { symbol_id } => {
                write!(f, "Invalid symbol ID: {}", symbol_id)
            }
            OrderError::SymbolMismatch { expected, actual } => {
                write!(f, "Symbol mismatch: expected '{}', got '{}'", expected, actual)
            }
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

pub fn u64_to_decimal_string(amount: u64, decimals: u32) -> String {
    Decimal::from_i128_with_scale(amount as i128, decimals).to_string()
}
