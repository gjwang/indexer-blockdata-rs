
#[cfg(test)]
mod balance_field_tests {
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
    fn test_all_balance_fields_after_deposit() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Deposit 100,000 USDT
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000, balance_after: 0, version: 0 })
            .unwrap();

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        // Verify ALL fields
        assert_eq!(usdt.avail, 100000, "Available should be 100,000");
        assert_eq!(usdt.frozen, 0, "Frozen should be 0");
        assert_eq!(usdt.version, 1, "Version should be 1 after first deposit");

        // Verify total
        assert_eq!(usdt.avail + usdt.frozen, 100000, "Total must equal deposit");
    }

    #[test]
    fn test_all_balance_fields_after_lock() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Deposit then place order
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000, balance_after: 0, version: 0 }).unwrap();

        let version_before = engine.ledger.get_balance_version(1, 200);

        // Place order (locks 50,000)
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1000)]);

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        // Verify ALL fields after lock
        assert_eq!(usdt.avail, 50000, "Available should be 50,000 (100k - 50k locked)");
        assert_eq!(usdt.frozen, 50000, "Frozen should be 50,000");
        assert!(usdt.version > version_before, "Version must increment on lock");

        // Verify total unchanged
        assert_eq!(usdt.avail + usdt.frozen, 100000, "Total must remain 100,000");
    }

    #[test]
    fn test_all_balance_fields_buyer_after_full_trade() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund buyer with USDT
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000, balance_after: 0, version: 0 }).unwrap();

        // Fund seller with BTC
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 2, asset: 100, amount: 10, balance_after: 0, version: 0 }).unwrap();

        // Place sell order (maker)
        engine.add_order_batch(vec![(1, 201, Side::Sell, OrderType::Limit, 50000, 1, 2, 1000)]);

        // Record buyer's initial state
        let buyer_usdt_v0 = engine.ledger.get_balance_version(1, 200);
        let buyer_btc_v0 = engine.ledger.get_balance_version(1, 100);

        // Place buy order (taker) - matches
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1001)]);

        // Verify buyer's USDT (quote asset)
        let buyer_balances = engine.ledger.get_user_balances(1).unwrap();
        let buyer_usdt = buyer_balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(buyer_usdt.avail, 50000, "Buyer USDT available should be 50,000");
        assert_eq!(buyer_usdt.frozen, 0, "Buyer USDT frozen should be 0");
        assert!(buyer_usdt.version > buyer_usdt_v0, "Buyer USDT version must increment");
        assert_eq!(buyer_usdt.avail + buyer_usdt.frozen, 50000, "Buyer USDT total should be 50,000");

        // Verify buyer's BTC (base asset - gained)
        let buyer_btc = buyer_balances.iter().find(|(a, _)| *a == 100).unwrap().1;

        assert_eq!(buyer_btc.avail, 1, "Buyer BTC available should be 1");
        assert_eq!(buyer_btc.frozen, 0, "Buyer BTC frozen should be 0");
        assert!(buyer_btc.version > buyer_btc_v0, "Buyer BTC version must increment");
        assert_eq!(buyer_btc.avail + buyer_btc.frozen, 1, "Buyer BTC total should be 1");
    }

    #[test]
    fn test_all_balance_fields_seller_after_full_trade() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund users
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000, balance_after: 0, version: 0 }).unwrap();
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 2, asset: 100, amount: 10, balance_after: 0, version: 0 }).unwrap();

        // Record seller's initial state
        let seller_btc_v0 = engine.ledger.get_balance_version(2, 100);
        let seller_usdt_v0 = engine.ledger.get_balance_version(2, 200);

        // Place sell order (maker)
        engine.add_order_batch(vec![(1, 201, Side::Sell, OrderType::Limit, 50000, 1, 2, 1000)]);

        // Place buy order (taker)
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 1, 1, 1001)]);

        // Verify seller's BTC (base asset - spent)
        let seller_balances = engine.ledger.get_user_balances(2).unwrap();
        let seller_btc = seller_balances.iter().find(|(a, _)| *a == 100).unwrap().1;

        assert_eq!(seller_btc.avail, 9, "Seller BTC available should be 9");
        assert_eq!(seller_btc.frozen, 0, "Seller BTC frozen should be 0");
        assert!(seller_btc.version > seller_btc_v0, "Seller BTC version must increment");
        assert_eq!(seller_btc.avail + seller_btc.frozen, 9, "Seller BTC total should be 9");

        // Verify seller's USDT (quote asset - gained)
        let seller_usdt = seller_balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(seller_usdt.avail, 50000, "Seller USDT available should be 50,000");
        assert_eq!(seller_usdt.frozen, 0, "Seller USDT frozen should be 0");
        assert!(seller_usdt.version > seller_usdt_v0, "Seller USDT version must increment");
        assert_eq!(seller_usdt.avail + seller_usdt.frozen, 50000, "Seller USDT total should be 50,000");
    }

    #[test]
    fn test_all_balance_fields_partial_fill() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Fund users
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 200000, balance_after: 0, version: 0 }).unwrap();
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 2, asset: 100, amount: 10, balance_after: 0, version: 0 }).unwrap();

        // Buyer orders 4 BTC @ 50,000 = 200,000 USDT
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 50000, 4, 1, 1000)]);

        // Verify buyer's state after placing order
        let buyer_balances = engine.ledger.get_user_balances(1).unwrap();
        let buyer_usdt = buyer_balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(buyer_usdt.avail, 0, "All USDT should be locked");
        assert_eq!(buyer_usdt.frozen, 200000, "200,000 USDT should be frozen");
        assert_eq!(buyer_usdt.avail + buyer_usdt.frozen, 200000, "Total should be 200,000");

        // Seller sells 2 BTC (partial fill)
        engine.add_order_batch(vec![(1, 201, Side::Sell, OrderType::Limit, 50000, 2, 2, 1001)]);

        // Verify buyer's state after partial fill
        let buyer_balances = engine.ledger.get_user_balances(1).unwrap();
        let buyer_usdt = buyer_balances.iter().find(|(a, _)| *a == 200).unwrap().1;
        let buyer_btc = buyer_balances.iter().find(|(a, _)| *a == 100).unwrap().1;

        // Buyer should have:
        // - Spent 100,000 USDT (2 BTC * 50,000)
        // - Still have 100,000 USDT locked (for remaining 2 BTC order)
        assert_eq!(buyer_usdt.avail, 0, "Buyer USDT available should be 0");
        assert_eq!(buyer_usdt.frozen, 100000, "Buyer should still have 100,000 USDT locked");
        assert_eq!(buyer_usdt.avail + buyer_usdt.frozen, 100000, "Buyer USDT total should be 100,000");

        // Buyer should have gained 2 BTC
        assert_eq!(buyer_btc.avail, 2, "Buyer should have 2 BTC");
        assert_eq!(buyer_btc.frozen, 0, "Buyer BTC frozen should be 0");
        assert_eq!(buyer_btc.avail + buyer_btc.frozen, 2, "Buyer BTC total should be 2");

        // Verify seller's state
        let seller_balances = engine.ledger.get_user_balances(2).unwrap();
        let seller_btc = seller_balances.iter().find(|(a, _)| *a == 100).unwrap().1;
        let seller_usdt = seller_balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(seller_btc.avail, 8, "Seller should have 8 BTC left");
        assert_eq!(seller_btc.frozen, 0, "Seller BTC frozen should be 0");
        assert_eq!(seller_btc.avail + seller_btc.frozen, 8, "Seller BTC total should be 8");

        assert_eq!(seller_usdt.avail, 100000, "Seller should have gained 100,000 USDT");
        assert_eq!(seller_usdt.frozen, 0, "Seller USDT frozen should be 0");
        assert_eq!(seller_usdt.avail + seller_usdt.frozen, 100000, "Seller USDT total should be 100,000");
    }

    #[test]
    fn test_balance_fields_zero_state() {
        let (engine, _wal_dir, _snap_dir) = create_test_engine();

        // Query non-existent user
        let balance = engine.ledger.get_balance(999, 200);
        assert_eq!(balance, 0, "Non-existent user should have 0 balance");

        let version = engine.ledger.get_balance_version(999, 200);
        assert_eq!(version, 0, "Non-existent user should have version 0");

        let balances = engine.ledger.get_user_balances(999);
        assert!(balances.is_none(), "Non-existent user should return None");
    }

    #[test]
    fn test_balance_fields_multiple_deposits() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // First deposit
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 50000, balance_after: 0, version: 0 }).unwrap();

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt.avail, 50000, "First deposit: available should be 50,000");
        assert_eq!(usdt.frozen, 0, "First deposit: frozen should be 0");
        let version_1 = usdt.version;

        // Second deposit
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 30000, balance_after: 0, version: 0 }).unwrap();

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt.avail, 80000, "Second deposit: available should be 80,000");
        assert_eq!(usdt.frozen, 0, "Second deposit: frozen should be 0");
        assert!(usdt.version > version_1, "Version must increment on second deposit");

        // Third deposit
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 20000, balance_after: 0, version: 0 }).unwrap();

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt.avail, 100000, "Third deposit: available should be 100,000");
        assert_eq!(usdt.frozen, 0, "Third deposit: frozen should be 0");
        assert_eq!(usdt.avail + usdt.frozen, 100000, "Total should be 100,000");
    }

    #[test]
    fn test_balance_fields_after_multiple_locks() {
        let (mut engine, _wal_dir, _snap_dir) = create_test_engine();

        engine.register_symbol(1, "BTC_USDT".to_string(), 100, 200).unwrap();

        // Deposit 100,000
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 200, amount: 100000, balance_after: 0, version: 0 }).unwrap();

        // Place first order (locks 30,000)
        engine.add_order_batch(vec![(1, 101, Side::Buy, OrderType::Limit, 30000, 1, 1, 1000)]);

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt.avail, 70000, "After first order: available should be 70,000");
        assert_eq!(usdt.frozen, 30000, "After first order: frozen should be 30,000");

        // Place second order (locks 40,000 more)
        engine.add_order_batch(vec![(1, 102, Side::Buy, OrderType::Limit, 40000, 1, 1, 1001)]);

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt.avail, 30000, "After second order: available should be 30,000");
        assert_eq!(usdt.frozen, 70000, "After second order: frozen should be 70,000");

        // Place third order (locks remaining 30,000)
        engine.add_order_batch(vec![(1, 103, Side::Buy, OrderType::Limit, 30000, 1, 1, 1002)]);

        let balances = engine.ledger.get_user_balances(1).unwrap();
        let usdt = balances.iter().find(|(a, _)| *a == 200).unwrap().1;

        assert_eq!(usdt.avail, 0, "After third order: available should be 0");
        assert_eq!(usdt.frozen, 100000, "After third order: frozen should be 100,000");
        assert_eq!(usdt.avail + usdt.frozen, 100000, "Total must remain 100,000");
    }
}
