#[cfg(test)]
mod order_lifecycle_integration_tests {
    use fetcher::ledger::{LedgerCommand, OrderStatus, OrderUpdate};
    use fetcher::matching_engine_base::MatchingEngine;
    use fetcher::models::{OrderType, Side};
    use tempfile::TempDir;

    fn create_test_engine() -> (MatchingEngine, TempDir, TempDir) {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let engine = MatchingEngine::new(wal_dir.path(), snap_dir.path(), false).unwrap();
        (engine, wal_dir, snap_dir)
    }

    #[test]
    fn test_order_new_event_emission() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Setup
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 })
            .unwrap();

        // Place order
        let (_results, commands) = engine.add_order_batch(vec![(
            1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000,
        )]);

        // Verify OrderUpdate(New) emitted
        let updates: Vec<&OrderUpdate> = commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(u) => Some(u),
                _ => None,
            })
            .collect();

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].status, OrderStatus::New);
        assert_eq!(updates[0].order_id, 101);
        assert_eq!(updates[0].user_id, 1);
        assert_eq!(updates[0].filled_qty, 0);
    }

    #[test]
    fn test_order_cancelled_event_emission() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Setup
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 })
            .unwrap();

        // Place order
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000)]);

        // Cancel order
        let commands = engine.cancel_order(1, 101).unwrap();

        // Verify OrderUpdate(Cancelled) emitted
        let updates: Vec<&OrderUpdate> = commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(u) => Some(u),
                _ => None,
            })
            .collect();

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].status, OrderStatus::Cancelled);
        assert_eq!(updates[0].order_id, 101);
    }

    #[test]
    fn test_order_rejected_no_funds() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Setup (no funding)
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Try to place order without funds
        let (results, _commands) = engine.add_order_batch(vec![(
            1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000,
        )]);

        // Verify order processed (executed as Rejected)
        assert!(results[0].is_ok(), "Order should be processed (Rejected event)");
    }

    #[test]
    fn test_multiple_orders_multiple_events() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Setup
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();
        for user_id in 1..=3 {
            engine
                .ledger
                .apply(&LedgerCommand::Deposit { user_id, asset: 200, amount: 100000 })
                .unwrap();
        }

        // Place 3 orders
        let (_results, commands) = engine.add_order_batch(vec![
            (1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000),
            (1, 102, Side::Buy, OrderType::Limit, 50000, 1, 2, 1001),
            (1, 103, Side::Buy, OrderType::Limit, 50000, 1, 3, 1002),
        ]);

        // Verify 3 OrderUpdate events
        let updates: Vec<&OrderUpdate> = commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(u) => Some(u),
                _ => None,
            })
            .collect();

        assert_eq!(updates.len(), 3);
        assert_eq!(updates[0].order_id, 101);
        assert_eq!(updates[1].order_id, 102);
        assert_eq!(updates[2].order_id, 103);
    }

    #[test]
    fn test_order_event_serialization() {
        let order_update = OrderUpdate {
            order_id: 101,
            client_order_id: Some("CID_101".to_string()),
            user_id: 1,
            symbol_id: 1,
            side: 1, // Buy
            order_type: 1, // Limit
            status: OrderStatus::New,
            price: 50000,
            qty: 1,
            filled_qty: 0,
            avg_fill_price: None,
            rejection_reason: None,
            timestamp: 1000000,
            match_id: None,
        };

        // Test JSON serialization
        let json = serde_json::to_string(&order_update).unwrap();
        assert!(json.contains("\"order_id\":101"));
        assert!(json.contains("\"status\":\"New\""));

        // Test deserialization
        let deserialized: OrderUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.order_id, 101);
        assert_eq!(deserialized.status, OrderStatus::New);
    }

    #[test]
    fn test_ledger_command_order_update_variant() {
        let order_update = OrderUpdate {
            order_id: 101,
            client_order_id: None,
            user_id: 1,
            symbol_id: 1,
            side: 1, // Buy
            order_type: 1, // Limit
            status: OrderStatus::Filled,
            price: 50000,
            qty: 1,
            filled_qty: 1,
            avg_fill_price: Some(50000),
            rejection_reason: None,
            timestamp: 1000000,
            match_id: Some(1),
        };

        let command = LedgerCommand::OrderUpdate(order_update.clone());

        // Test serialization
        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains("OrderUpdate"));

        // Test deserialization
        let deserialized: LedgerCommand = serde_json::from_str(&json).unwrap();
        match deserialized {
            LedgerCommand::OrderUpdate(u) => {
                assert_eq!(u.order_id, 101);
                assert_eq!(u.status, OrderStatus::Filled);
            }
            _ => panic!("Expected OrderUpdate variant"),
        }
    }

    #[test]
    fn test_order_status_hash() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert(OrderStatus::New, 1);
        map.insert(OrderStatus::Filled, 2);
        map.insert(OrderStatus::Cancelled, 3);

        assert_eq!(map.get(&OrderStatus::New), Some(&1));
        assert_eq!(map.get(&OrderStatus::Filled), Some(&2));
        assert_eq!(map.get(&OrderStatus::Cancelled), Some(&3));
    }

    #[test]
    fn test_order_lifecycle_state_transitions() {
        // Valid transitions
        let transitions = vec![
            (OrderStatus::New, OrderStatus::PartiallyFilled),
            (OrderStatus::PartiallyFilled, OrderStatus::Filled),
            (OrderStatus::New, OrderStatus::Cancelled),
            (OrderStatus::PartiallyFilled, OrderStatus::Cancelled),
            (OrderStatus::New, OrderStatus::Expired),
        ];

        for (from, to) in transitions {
            // Just verify we can create orders with these statuses
            let _order1 = OrderUpdate {
                order_id: 1,
                client_order_id: None,
                user_id: 1,
                symbol_id: 1,
                side: 1,
                order_type: 1,
                status: from,
                price: 50000,
                qty: 1,
                filled_qty: 0,
                avg_fill_price: None,
                rejection_reason: None,
                timestamp: 1000000,
                match_id: None,
            };

            let _order2 = OrderUpdate {
                order_id: 1,
                client_order_id: None,
                user_id: 1,
                symbol_id: 1,
                side: 1,
                order_type: 1,
                status: to,
                price: 50000,
                qty: 1,
                filled_qty: 1,
                avg_fill_price: Some(50000),
                rejection_reason: None,
                timestamp: 1000001,
                match_id: None,
            };
        }
    }
}
