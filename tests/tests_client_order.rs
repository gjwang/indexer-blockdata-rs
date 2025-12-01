#[cfg(test)]
mod tests {
    use fetcher::models::{ClientOrder, OrderRequest, OrderType, Side};
    use fetcher::symbol_manager::SymbolManager;

    fn setup_symbol_manager() -> SymbolManager {
        let mut sm = SymbolManager::new();
        sm.insert("BTC_USDT", 1);
        sm.insert("ETH_USDT", 2);
        sm
    }

    #[test]
    fn test_try_to_internal_success() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            symbol: "BTC_USDT".to_string(),
            side: "Buy".to_string(),
            price: 50000,
            quantity: 100,
            user_id: 1,
            order_type: "Limit".to_string(),
        };

        let result = client_order.try_to_internal(&sm, 1001);
        assert!(result.is_ok());
        if let Ok(OrderRequest::PlaceOrder {
            symbol_id,
            side,
            price,
            quantity,
            order_type,
            ..
        }) = result
        {
            assert_eq!(symbol_id, 1);
            assert_eq!(side, Side::Buy);
            assert_eq!(price, 50000);
            assert_eq!(quantity, 100);
            assert_eq!(order_type, OrderType::Limit);
        } else {
            panic!("Unexpected result type");
        }
    }

    #[test]
    fn test_try_to_internal_unknown_symbol() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            symbol: "UNKNOWN".to_string(),
            side: "Buy".to_string(),
            price: 50000,
            quantity: 100,
            user_id: 1,
            order_type: "Limit".to_string(),
        };

        let result = client_order.try_to_internal(&sm, 1001);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Unknown symbol: UNKNOWN");
    }

    #[test]
    fn test_try_to_internal_invalid_side() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            symbol: "BTC_USDT".to_string(),
            side: "Invalid".to_string(),
            price: 50000,
            quantity: 100,
            user_id: 1,
            order_type: "Limit".to_string(),
        };

        let result = client_order.try_to_internal(&sm, 1001);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid side"));
    }

    #[test]
    fn test_try_to_internal_invalid_order_type() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            symbol: "BTC_USDT".to_string(),
            side: "Buy".to_string(),
            price: 50000,
            quantity: 100,
            user_id: 1,
            order_type: "Invalid".to_string(),
        };

        let result = client_order.try_to_internal(&sm, 1001);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid order type"));
    }

    #[test]
    fn test_try_to_internal_invalid_price() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            symbol: "BTC_USDT".to_string(),
            side: "Buy".to_string(),
            price: 0,
            quantity: 100,
            user_id: 1,
            order_type: "Limit".to_string(),
        };

        let result = client_order.try_to_internal(&sm, 1001);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Price must be greater than 0");
    }

    #[test]
    fn test_try_to_internal_invalid_quantity() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            symbol: "BTC_USDT".to_string(),
            side: "Buy".to_string(),
            price: 50000,
            quantity: 0,
            user_id: 1,
            order_type: "Limit".to_string(),
        };

        let result = client_order.try_to_internal(&sm, 1001);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Quantity must be greater than 0");
    }

    #[test]
    fn test_try_from_internal_success() {
        let sm = setup_symbol_manager();
        let request = OrderRequest::PlaceOrder {
            order_id: 1001,
            user_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 60000,
            quantity: 50,
            order_type: OrderType::Market,
        };

        let result = ClientOrder::try_from_internal(&request, &sm);
        assert!(result.is_ok());
        let client_order = result.unwrap();
        assert_eq!(client_order.symbol, "BTC_USDT");
        assert_eq!(client_order.side, "Sell");
        assert_eq!(client_order.price, 60000);
        assert_eq!(client_order.quantity, 50);
        assert_eq!(client_order.order_type, "Market");
    }

    #[test]
    fn test_try_from_internal_unknown_symbol_id() {
        let sm = setup_symbol_manager();
        let request = OrderRequest::PlaceOrder {
            order_id: 1001,
            user_id: 1,
            symbol_id: 999, // Unknown ID
            side: Side::Sell,
            price: 60000,
            quantity: 50,
            order_type: OrderType::Market,
        };

        let result = ClientOrder::try_from_internal(&request, &sm);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Unknown symbol ID: 999");
    }

    #[test]
    fn test_try_from_internal_invalid_request_type() {
        let sm = setup_symbol_manager();
        let request = OrderRequest::CancelOrder {
            order_id: 1001,
            user_id: 1,
            symbol_id: 1,
        };

        let result = ClientOrder::try_from_internal(&request, &sm);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Only PlaceOrder can be converted to ClientOrder"
        );
    }
}
