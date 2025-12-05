#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{OrderStatus, OrderUpdate};

    #[test]
    fn test_order_update_json_serialization() {
        let update = OrderUpdate {
            order_id: 12345,
            client_order_id: Some("cid_001".to_string()),
            user_id: 1,
            symbol: "BTC_USDT".to_string(),
            side: 1, // Buy
            order_type: 1, // Limit
            status: OrderStatus::Filled,
            price: 50000,
            qty: 100,
            filled_qty: 100,
            avg_fill_price: Some(50000),
            rejection_reason: None,
            timestamp: 1622540000000,
            match_id: Some(999),
        };

        let json = serde_json::to_string(&update).expect("Serialization failed");
        let deserialized: OrderUpdate = serde_json::from_str(&json).expect("Deserialization failed");

        assert_eq!(deserialized.order_id, 12345);
        assert_eq!(deserialized.status, OrderStatus::Filled);
        assert_eq!(deserialized.avg_fill_price, Some(50000));
        assert_eq!(deserialized.match_id, Some(999));
    }

    #[test]
    fn test_order_update_bincode_serialization() {
        let update = OrderUpdate {
            order_id: 67890,
            client_order_id: None,
            user_id: 2,
            symbol: "ETH_USDT".to_string(),
            side: 2, // Sell
            order_type: 2, // Market
            status: OrderStatus::New,
            price: 3000,
            qty: 10,
            filled_qty: 0,
            avg_fill_price: None,
            rejection_reason: None,
            timestamp: 1622540000000,
            match_id: None,
        };

        let encoded = bincode::serialize(&update).expect("Serialization failed");
        let decoded: OrderUpdate = bincode::deserialize(&encoded).expect("Deserialization failed");

        assert_eq!(decoded.order_id, 67890);
        assert_eq!(decoded.status, OrderStatus::New);
        assert!(decoded.avg_fill_price.is_none());
    }
}
