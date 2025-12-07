#[cfg(test)]
mod order_lifecycle_tests {
    use crate::ledger::{LedgerCommand, OrderStatus, OrderUpdate};
    use crate::matching_engine_base::MatchingEngine;
    use crate::models::{OrderError, OrderType, Side};
    use tempfile::TempDir;

    fn create_test_engine() -> (MatchingEngine, TempDir, TempDir) {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let engine = MatchingEngine::new(wal_dir.path(), snap_dir.path(), false).unwrap();
        (engine, wal_dir, snap_dir)
    }

    #[test]
    fn test_order_lifecycle_emission_new() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Register symbol
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user
        engine
            .ledger
            .apply(&LedgerCommand::Deposit {
                user_id: 1,
                asset_id: 200,
                amount: 100000,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Place order
        let (results, commands) = engine.add_order_batch(vec![(
            1,     // symbol_id
            12345, // order_id
            Side::Buy,
            OrderType::Limit,
            50000, // price
            1,     // quantity
            1,     // user_id
            1000,  // timestamp
        )]);

        // Verify order was accepted
        assert!(results[0].is_ok());

        // Verify OrderUpdate(New) was emitted
        let order_updates: Vec<&OrderUpdate> = commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(update) => Some(update),
                _ => None,
            })
            .collect();

        assert_eq!(order_updates.len(), 1, "Should emit exactly one OrderUpdate");
        let update = order_updates[0];
        assert_eq!(update.order_id, 12345);
        assert_eq!(update.status, OrderStatus::New);
        assert_eq!(update.user_id, 1);
        assert_eq!(update.filled_qty, 0);
    }

    #[test]
    fn test_order_rejection_insufficient_funds() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Register symbol
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Don't fund user - should cause rejection

        // Attempt to place order
        let (results, _commands) = engine.add_order_batch(vec![(
            1,     // symbol_id
            12345, // order_id
            Side::Buy,
            OrderType::Limit,
            50000, // price
            1,     // quantity
            1,     // user_id
            1000,  // timestamp
        )]);

        // Verify order was accepted for processing (result Ok)
        assert!(results[0].is_ok(), "Should return Ok but emit Rejected event");

        // Verify OrderUpdate(Rejected) was emitted
        let order_updates: Vec<&OrderUpdate> = _commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(update) => Some(update),
                _ => None,
            })
            .collect();

        assert_eq!(order_updates.len(), 1, "Should emit exactly one OrderUpdate");
        let update = order_updates[0];
        assert_eq!(update.order_id, 12345);
        assert_eq!(update.status, OrderStatus::Rejected);
        assert!(update.rejection_reason.is_some());

        let reason = update.rejection_reason.as_ref().unwrap();
        assert!(
            reason.contains("Insufficient funds") || reason.contains("LedgerError"),
            "Reason was: {}",
            reason
        );
    }

    #[test]
    fn test_multiple_orders_emit_multiple_updates() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Register symbol
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund users
        for user_id in 1..=3 {
            engine
                .ledger
                .apply(&LedgerCommand::Deposit {
                    user_id,
                    asset_id: 200,
                    amount: 100000,
                    balance_after: 0,
                    version: 0,
                })
                .unwrap();
        }

        // Place multiple orders
        let (results, commands) = engine.add_order_batch(vec![
            (1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000),
            (1, 102, Side::Buy, OrderType::Limit, 50000, 1, 2, 1001),
            (1, 103, Side::Buy, OrderType::Limit, 50000, 1, 3, 1002),
        ]);

        // Verify all orders accepted
        assert!(results.iter().all(|r| r.is_ok()));

        // Verify OrderUpdate events
        let order_updates: Vec<&OrderUpdate> = commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(update) => Some(update),
                _ => None,
            })
            .collect();

        assert_eq!(order_updates.len(), 3, "Should emit 3 OrderUpdate events");
        assert_eq!(order_updates[0].order_id, 101);
        assert_eq!(order_updates[1].order_id, 102);
        assert_eq!(order_updates[2].order_id, 103);
    }

    #[test]
    fn test_order_cancellation_emits_update() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Register symbol
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user
        engine
            .ledger
            .apply(&LedgerCommand::Deposit {
                user_id: 1,
                asset_id: 200,
                amount: 100000,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Place order
        let (_results, _commands) =
            engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000)]);

        // Cancel order
        let cancel_commands = engine.cancel_order(1, 101).expect("Cancel should succeed");

        // Verify OrderUpdate(Cancelled) was emitted
        let order_updates: Vec<&OrderUpdate> = cancel_commands
            .iter()
            .filter_map(|cmd| match cmd {
                LedgerCommand::OrderUpdate(update) => Some(update),
                _ => None,
            })
            .collect();

        assert_eq!(order_updates.len(), 1, "Should emit exactly one OrderUpdate");
        let update = order_updates[0];
        assert_eq!(update.order_id, 101);
        assert_eq!(update.status, OrderStatus::Cancelled);
        assert_eq!(update.user_id, 1);
    }

    #[test]
    fn test_order_cancellation_unlocks_funds() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user with 100,000 USDT
        engine
            .ledger
            .apply(&LedgerCommand::Deposit {
                user_id: 1,
                asset_id: 200,
                amount: 100000,
                balance_after: 0,
                version: 0,
            })
            .unwrap();

        // Place order (locks 50,000 USDT)
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000)]);

        // Verify funds are locked
        let balances_before = engine.ledger.get_user_balances(1).unwrap();
        let usdt_before = balances_before.iter().find(|(a, _)| *a == 200).unwrap().1;
        assert_eq!(usdt_before.frozen, 50000, "50,000 should be locked");
        assert_eq!(usdt_before.avail, 50000, "50,000 should be available");

        // Cancel order
        let cancel_commands = engine.cancel_order(1, 101).expect("Cancel should succeed");

        // Verify Unlock command was emitted
        let unlock_commands: Vec<&LedgerCommand> = cancel_commands
            .iter()
            .filter(|cmd| matches!(cmd, LedgerCommand::Unlock { .. }))
            .collect();

        assert_eq!(unlock_commands.len(), 1, "Should emit one Unlock command");

        // Verify funds are unlocked
        let balances_after = engine.ledger.get_user_balances(1).unwrap();
        let usdt_after = balances_after.iter().find(|(a, _)| *a == 200).unwrap().1;
        assert_eq!(usdt_after.frozen, 0, "All funds should be unlocked");
        assert_eq!(usdt_after.avail, 100000, "All funds should be available");
    }

    #[test]
    fn test_cancel_nonexistent_order_fails() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Try to cancel non-existent order
        let result = engine.cancel_order(1, 9999);

        assert!(result.is_err(), "Cancelling non-existent order should fail");
    }
}
