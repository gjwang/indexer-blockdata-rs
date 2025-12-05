
#[cfg(test)]
mod balance_correctness_tests {
    use crate::matching_engine_base::MatchingEngine;
    use crate::ledger::{Ledger, LedgerCommand};
    use crate::models::{OrderType, Side};
    use tempfile::TempDir;

    fn create_test_engine() -> (MatchingEngine, TempDir, TempDir) {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let engine = MatchingEngine::new(wal_dir.path(), snap_dir.path(), false).unwrap();
        (engine, wal_dir, snap_dir)
    }

    #[test]
    fn test_balance_lock_correctness() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        // Register symbol: BTC_USDT (base=100, quote=200)
        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user with 100,000 USDT
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 })
            .unwrap();

        // Verify initial balance
        let initial_balance = engine.ledger.get_balance(1, 200);
        assert_eq!(initial_balance, 100000, "Initial balance should be 100,000");

        // Place buy order: 1 BTC @ 50,000 USDT = 50,000 USDT locked
        let (_results, _commands) = engine.add_order_batch(vec![(
            1,      // symbol_id
            101,    // order_id
            Side::Buy,
            OrderType::Limit,
            50000,  // price
            1,      // quantity
            1,      // user_id
            1000,   // timestamp
        )]);

        // Verify balance after lock
        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt_balance = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt_balance.avail, 50000, "Available should be 50,000 (100,000 - 50,000 locked)");
        assert_eq!(usdt_balance.frozen, 50000, "Frozen should be 50,000");
        assert_eq!(
            usdt_balance.avail + usdt_balance.frozen,
            100000,
            "Total balance must be preserved"
        );
    }

    #[test]
    fn test_balance_version_increments_on_lock() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 })
            .unwrap();

        let initial_version = engine.ledger.get_balance_version(1, 200);

        // Place order (locks funds)
        let (_results, _commands) = engine.add_order_batch(vec![(
            1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000,
        )]);

        let new_version = engine.ledger.get_balance_version(1, 200);

        assert!(
            new_version > initial_version,
            "Version must increment on lock. Initial: {}, New: {}",
            initial_version,
            new_version
        );
    }

    #[test]
    fn test_balance_correctness_after_trade() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund buyer with USDT
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 })
            .unwrap();

        // Fund seller with BTC
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 2, asset: 100, amount: 10 })
            .unwrap();

        // Place sell order first (maker)
        let (_results, _commands) = engine.add_order_batch(vec![(
            1, 201, Side::Sell, OrderType::Limit, 50000, 1, 2, 1000,
        )]);

        // Verify seller's BTC is locked
        let seller_btc = engine.ledger.get_user_balances(2).unwrap();
        let seller_btc_bal = seller_btc.iter().find(|(a, _)| *a == 100).unwrap().1;
        assert_eq!(seller_btc_bal.avail, 9, "Seller should have 9 BTC available");
        assert_eq!(seller_btc_bal.frozen, 1, "Seller should have 1 BTC frozen");

        // Place buy order (taker) - should match
        let (_results, _commands) = engine.add_order_batch(vec![(
            1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1001,
        )]);

        // Verify buyer's balances after trade
        let buyer_balances = engine.ledger.get_user_balances(1).unwrap();
        let buyer_usdt = buyer_balances.iter().find(|(a, _)| *a == 200).unwrap().1;
        let buyer_btc = buyer_balances.iter().find(|(a, _)| *a == 100).unwrap().1;

        assert_eq!(buyer_usdt.avail, 50000, "Buyer should have 50,000 USDT left");
        assert_eq!(buyer_usdt.frozen, 0, "Buyer should have 0 USDT frozen");
        assert_eq!(buyer_btc.avail, 1, "Buyer should have gained 1 BTC");
        assert_eq!(buyer_btc.frozen, 0, "Buyer should have 0 BTC frozen");

        // Verify seller's balances after trade
        let seller_balances = engine.ledger.get_user_balances(2).unwrap();
        let seller_btc = seller_balances.iter().find(|(a, _)| *a == 100).unwrap().1;
        let seller_usdt = seller_balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(seller_btc.avail, 9, "Seller should have 9 BTC left");
        assert_eq!(seller_btc.frozen, 0, "Seller should have 0 BTC frozen");
        assert_eq!(seller_usdt.avail, 50000, "Seller should have gained 50,000 USDT");
        assert_eq!(seller_usdt.frozen, 0, "Seller should have 0 USDT frozen");
    }

    #[test]
    fn test_balance_version_increments_on_trade() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund users
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 }).unwrap();
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 2, asset: 100, amount: 10 }).unwrap();

        // Record initial versions
        let buyer_usdt_v0 = engine.ledger.get_balance_version(1, 200);
        let buyer_btc_v0 = engine.ledger.get_balance_version(1, 100);
        let seller_btc_v0 = engine.ledger.get_balance_version(2, 100);
        let seller_usdt_v0 = engine.ledger.get_balance_version(2, 200);

        // Place sell order (locks seller's BTC)
        engine.add_order_batch(vec![(1, 201, Side::Sell, OrderType::Limit, 50000, 1, 2, 1000)]);

        let seller_btc_v1 = engine.ledger.get_balance_version(2, 100);
        assert!(seller_btc_v1 > seller_btc_v0, "Seller BTC version must increment on lock");

        // Place buy order (matches)
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1001)]);

        // Verify all versions incremented
        let buyer_usdt_v1 = engine.ledger.get_balance_version(1, 200);
        let buyer_btc_v1 = engine.ledger.get_balance_version(1, 100);
        let seller_btc_v2 = engine.ledger.get_balance_version(2, 100);
        let seller_usdt_v1 = engine.ledger.get_balance_version(2, 200);

        assert!(buyer_usdt_v1 > buyer_usdt_v0, "Buyer USDT version must increment (lock + spend)");
        assert!(buyer_btc_v1 > buyer_btc_v0, "Buyer BTC version must increment (gain)");
        assert!(seller_btc_v2 > seller_btc_v1, "Seller BTC version must increment again (spend)");
        assert!(seller_usdt_v1 > seller_usdt_v0, "Seller USDT version must increment (gain)");
    }

    #[test]
    fn test_partial_fill_balance_correctness() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund users
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 200000 }).unwrap();
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 2, asset: 100, amount: 10 }).unwrap();

        // Buyer places order for 5 BTC @ 50,000 = 250,000 USDT
        // But only has 200,000, so this should lock 200,000
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 4, 1, 1000)]);

        let buyer_usdt = engine.ledger.get_user_balances(1).unwrap();
        let buyer_usdt_bal = buyer_usdt.iter().find(|(a, _)| *a == 200).unwrap().1;
        assert_eq!(buyer_usdt_bal.frozen, 200000, "Should lock exactly 200,000 USDT");

        // Seller places order for 2 BTC @ 50,000 (partial fill)
        engine.add_order_batch(vec![(1, 201, Side::Sell, OrderType::Limit, 50000, 2, 2, 1001)]);

        // Verify buyer got 2 BTC and spent 100,000 USDT
        let buyer_balances = engine.ledger.get_user_balances(1).unwrap();
        let buyer_usdt = buyer_balances.iter().find(|(a, _)| *a == 200).unwrap().1;
        let buyer_btc = buyer_balances.iter().find(|(a, _)| *a == 100).unwrap().1;

        assert_eq!(buyer_btc.avail, 2, "Buyer should have 2 BTC");
        assert_eq!(buyer_usdt.frozen, 100000, "Buyer should still have 100,000 USDT locked for remaining order");
        assert_eq!(buyer_usdt.avail, 0, "Buyer should have 0 USDT available");

        // Total should still be 200,000
        assert_eq!(
            buyer_usdt.avail + buyer_usdt.frozen,
            100000,
            "Buyer total USDT should be 100,000 (200,000 - 100,000 spent)"
        );
    }

    #[test]
    fn test_no_balance_leak_on_multiple_orders() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user with exactly 100,000 USDT
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000 }).unwrap();

        // Place 10 orders of 10,000 USDT each
        for i in 0..10 {
            let (_results, _commands) = engine.add_order_batch(vec![(
                1,
                100 + i,
                Side::Buy,
                OrderType::Limit,
                10000,
                1,
                1,
                1000 + i,
            )]);
        }

        // Verify total balance is still exactly 100,000
        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt_balance = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        let total = usdt_balance.avail + usdt_balance.frozen;
        assert_eq!(total, 100000, "Total balance must remain exactly 100,000. Got: {}", total);
        assert_eq!(usdt_balance.frozen, 100000, "All funds should be locked");
        assert_eq!(usdt_balance.avail, 0, "No funds should be available");
    }

    #[test]
    fn test_balance_invariant_after_failed_order() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund user with 50,000 USDT
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 50000 }).unwrap();

        let initial_balance = engine.ledger.get_balance(1, 200);
        let initial_version = engine.ledger.get_balance_version(1, 200);

        // Try to place order requiring 100,000 USDT (should fail)
        let (results, _commands) = engine.add_order_batch(vec![(
            1, 101, Side::Buy, OrderType::Limit, 100000, 1, 1, 1000,
        )]);

        assert!(results[0].is_err(), "Order should fail due to insufficient funds");

        // Verify balance unchanged
        let final_balance = engine.ledger.get_balance(1, 200);
        let final_version = engine.ledger.get_balance_version(1, 200);

        assert_eq!(final_balance, initial_balance, "Balance must not change on failed order");
        assert_eq!(final_version, initial_version, "Version must not change on failed order");
    }

    #[test]
    fn test_concurrent_balance_operations_correctness() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund multiple users
        for user_id in 1..=5 {
            engine.ledger.apply(&LedgerCommand::Deposit {
                user_id,
                asset: 200,
                amount: 100000,
            }).unwrap();
        }

        // Place orders from all users in a single batch
        let orders: Vec<_> = (1..=5)
            .map(|user_id| (1, 100 + user_id, Side::Buy, OrderType::Limit, 50000, 1, user_id, 1000))
            .collect();

        let (_results, _commands) = engine.add_order_batch(orders);

        // Verify each user's balance independently
        for user_id in 1..=5 {
            let balances = engine.ledger.get_user_balances(user_id).unwrap();
            let usdt_balance = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

            assert_eq!(
                usdt_balance.avail + usdt_balance.frozen,
                100000,
                "User {} total balance must be preserved",
                user_id
            );
            assert_eq!(
                usdt_balance.frozen,
                50000,
                "User {} should have 50,000 frozen",
                user_id
            );
        }
    }
}
