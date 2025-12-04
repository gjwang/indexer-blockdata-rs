#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use fetcher::client_order_convertor::client_order_convert;
    use fetcher::fast_ulid::SnowflakeGenRng;
    use fetcher::models::{ClientOrder, OrderRequest, OrderType, Side};
    use fetcher::symbol_manager::SymbolManager;
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use std::sync::Mutex;

    fn setup_symbol_manager() -> SymbolManager {
        let mut sm = SymbolManager::new();
        sm.add_asset(1, 8); // BTC
        sm.add_asset(2, 8); // USDT
        sm.add_asset(3, 8); // ETH
        sm.insert("BTC_USDT", 1, 1, 2);
        sm.insert("ETH_USDT", 2, 3, 2);
        sm
    }

    #[test]
    fn test_try_to_internal_success() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.50").unwrap(),
            quantity: Decimal::from_str("1.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_ok(), "Error: {:?}", result.err());
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
            assert_eq!(price, 5000050); // 50000.50 * 100 (2 decimals)
            assert_eq!(quantity, 150000000); // 1.5 * 100000000 (8 decimals)
            assert_eq!(order_type, OrderType::Limit);
        } else {
            panic!("Unexpected result type");
        }
    }

    #[test]
    fn test_try_to_internal_unknown_symbol() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("clientid2234567890123".to_string()),
            symbol: "UNKNOWN_SYMBOL".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Unknown symbol: UNKNOWN_SYMBOL");
    }

    // Note: test_try_to_internal_invalid_side is no longer applicable
    // since side is now a Side enum which cannot be invalid at compile time

    // Note: test_try_to_internal_invalid_order_type is no longer applicable
    // since order_type is now an OrderType enum which cannot be invalid at compile time

    #[test]
    fn test_try_to_internal_invalid_price() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("clientid5234567890123".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("0").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Price must be a positive number") || err.contains("invalid_price"));
    }

    #[test]
    fn test_try_to_internal_invalid_quantity() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("clientid6234567890123".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("0").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("Quantity must be a positive number") || err.contains("invalid_quantity")
        );
    }

    #[test]
    fn test_try_from_internal_success() {
        let sm = setup_symbol_manager();
        let request = OrderRequest::PlaceOrder {
            order_id: 1001,
            user_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 350075,      // 3500.75 * 100 (2 decimals)
            quantity: 12345678, // 0.12345678 * 100000000 (8 decimals)
            order_type: OrderType::Market,
            checksum: 0,
        };

        let result = ClientOrder::try_from_internal(&request, &sm);
        assert!(result.is_ok());
        let client_order = result.unwrap();
        assert_eq!(client_order.symbol, "BTC_USDT");
        assert_eq!(client_order.side, Side::Sell);
        assert_eq!(client_order.price, Decimal::from_str("3500.75").unwrap());
        assert_eq!(
            client_order.quantity,
            Decimal::from_str("0.12345678").unwrap()
        );
        assert_eq!(client_order.order_type, OrderType::Market);
        // We expect an empty string for cid as it's not in OrderRequest
        assert_eq!(client_order.cid, None);
    }

    #[test]
    fn test_try_from_internal_unknown_symbol_id() {
        let sm = setup_symbol_manager();
        let request = OrderRequest::PlaceOrder {
            order_id: 1001,
            user_id: 1,
            symbol_id: 999, // Unknown ID
            side: Side::Sell,
            price: 350075,
            quantity: 12345678,
            order_type: OrderType::Market,
            checksum: 0,
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
            checksum: 0,
        };

        let result = ClientOrder::try_from_internal(&request, &sm);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Only PlaceOrder can be converted to ClientOrder"
        );
    }

    #[test]
    fn test_try_to_internal_invalid_cid_length() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("a".repeat(33)),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("cid"));
        assert!(err.contains("characters"));
    }

    #[test]
    fn test_try_to_internal_invalid_cid_chars() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("invalidid1234567890123!".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("cid"));
        assert!(err.contains("alphanumeric"));
    }

    #[test]
    fn test_from_json_success() {
        let json = r#"{
            "cid": "clientid1234567890123",
            "symbol": "BTC_USDT",
            "side": "Buy",
            "price": "50000.99",
            "quantity": "2.5",
            "order_type": "Limit"
        }"#;

        let result = ClientOrder::from_json(json);
        assert!(result.is_ok());
        let order = result.unwrap();
        assert_eq!(order.cid, Some("clientid1234567890123".to_string()));
        assert_eq!(order.symbol, "BTC_USDT");
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.price, Decimal::from_str("50000.99").unwrap());
        assert_eq!(order.quantity, Decimal::from_str("2.5").unwrap());
        assert_eq!(order.order_type, OrderType::Limit);
    }

    #[test]
    fn test_from_json_invalid_json() {
        let json = r#"{ "invalid": "json" }"#;
        let result = ClientOrder::from_json(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_success() {
        let result = ClientOrder::new(
            Some("clientid1234567890123".to_string()),
            "BTC_USDT".to_string(),
            Side::Buy,
            Decimal::from_str("50000.99").unwrap(),
            Decimal::from_str("2.5").unwrap(),
            OrderType::Limit,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_success_no_cid() {
        let result = ClientOrder::new(
            None,
            "BTC_USDT".to_string(),
            Side::Buy,
            Decimal::from_str("50000.99").unwrap(),
            Decimal::from_str("2.5").unwrap(),
            OrderType::Limit,
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap().cid, None);
    }

    #[test]
    fn test_new_invalid() {
        let result = ClientOrder::new(
            Some("invalidid1234567890123!".to_string()),
            "BTC_USDT".to_string(),
            Side::Buy,
            Decimal::from_str("50000.99").unwrap(),
            Decimal::from_str("2.5").unwrap(),
            OrderType::Limit,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("cid"));
        assert!(err.contains("alphanumeric"));
    }

    #[test]
    fn test_try_to_internal_invalid_cid_min_length() {
        let sm = setup_symbol_manager();
        let client_order = ClientOrder {
            cid: Some("shortid".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order.try_to_internal(&sm, 1001, 1);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("cid"));
        assert!(err.contains("characters"));
    }

    #[test]
    fn test_try_to_internal_invalid_symbol_format() {
        let sm = setup_symbol_manager();

        // Short
        let order1 = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "AB".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };
        let res1 = order1.try_to_internal(&sm, 1, 1);
        assert!(res1.is_err());
        assert!(res1.unwrap_err().contains("symbol"));

        // Lowercase
        let order2 = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "btc_usdt".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };
        let res2 = order2.try_to_internal(&sm, 1, 1);
        assert!(res2.is_err());
        assert!(res2.unwrap_err().contains("uppercase"));

        // Invalid char
        let order3 = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "BTC-USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };
        let res3 = order3.try_to_internal(&sm, 1, 1);
        assert!(res3.is_err());
        assert!(res3.unwrap_err().contains("alphanumeric"));
    }

    #[test]
    fn test_try_to_internal_invalid_symbol_format_underscore() {
        let sm = setup_symbol_manager();

        // No underscore
        let order1 = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "BTCUSDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };
        let res1 = order1.try_to_internal(&sm, 1, 1);
        assert!(res1.is_err());
        assert!(res1.unwrap_err().contains("underscore"));

        // Leading underscore
        let order2 = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "_BTCUSDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };
        let res2 = order2.try_to_internal(&sm, 1, 1);
        assert!(res2.is_err());
        assert!(res2.unwrap_err().contains("start or end"));

        // Double underscore
        let order3 = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "BTC__USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };
        let res3 = order3.try_to_internal(&sm, 1, 1);
        assert!(res3.is_err());
        assert!(res3.unwrap_err().contains("consecutive"));
    }

    #[test]
    fn test_process_order_success() {
        let mut sm = SymbolManager::new();
        sm.add_asset(1, 8); // BTC
        sm.add_asset(2, 8); // USDT
        sm.insert("BTC_USDT", 1, 1, 2);
        let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

        let client_order = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order_convert(&client_order, &sm, &snowflake_gen, 1);
        assert!(result.is_ok());
        let (order_id, _internal_order) = result.unwrap();
        assert!(order_id > 0);
    }

    #[test]
    fn test_process_order_invalid_symbol() {
        let sm = SymbolManager::new(); // Empty
        let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

        let client_order = ClientOrder {
            cid: Some("clientid1234567890123".to_string()),
            symbol: "BTC_USDT".to_string(),
            side: Side::Buy,
            price: Decimal::from_str("50000.99").unwrap(),
            quantity: Decimal::from_str("2.5").unwrap(),
            order_type: OrderType::Limit,
        };

        let result = client_order_convert(&client_order, &sm, &snowflake_gen, 1);
        let err = result.unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("Unknown symbol"));
    }
    #[test]
    fn test_cid_valid_with_underscore() {
        let result = ClientOrder::new(
            Some("client_id_1234567890".to_string()),
            "BTC_USDT".to_string(),
            Side::Buy,
            Decimal::from_str("50000.99").unwrap(),
            Decimal::from_str("2.5").unwrap(),
            OrderType::Limit,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_cid_invalid_chars() {
        let result = ClientOrder::new(
            Some("client-id-1234567890".to_string()), // Hyphen not allowed
            "BTC_USDT".to_string(),
            Side::Buy,
            Decimal::from_str("50000.99").unwrap(),
            Decimal::from_str("2.5").unwrap(),
            OrderType::Limit,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("alphanumeric"));
    }
}
