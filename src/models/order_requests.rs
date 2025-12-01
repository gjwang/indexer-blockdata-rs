use crate::models::{OrderType, Side};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum OrderRequest {
    PlaceOrder {
        order_id: u64,
        user_id: u64,
        symbol_id: u32,
        side: Side,
        price: u64,
        quantity: u64,
        order_type: OrderType,
    },
    CancelOrder {
        order_id: u64,
        user_id: u64,
        symbol_id: u32,
    },
}
