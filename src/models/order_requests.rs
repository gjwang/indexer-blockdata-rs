use serde::{Deserialize, Serialize};

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
