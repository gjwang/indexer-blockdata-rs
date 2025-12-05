#[cfg(test)]
mod tests {
    use fetcher::ledger::LedgerCommand;
    use fetcher::matching_engine_base::MatchingEngine;
    use fetcher::models::{OrderError, OrderType, Side};
    use tempfile::TempDir;

    fn setup_engine(temp_dir: &TempDir) -> MatchingEngine {
        let wal_dir = temp_dir.path().join("wal");
        let snap_dir = temp_dir.path().join("snap");

        let mut engine = MatchingEngine::new(&wal_dir, &snap_dir, false).unwrap();

        // Register a symbol: BTC_USDT (ID 0), Base Asset 1, Quote Asset 2
        engine.register_symbol(0, "BTC_USDT".to_string(), 1, 2).unwrap();

        engine
    }

    #[test]
    fn test_add_order_sufficient_funds() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = setup_engine(&temp_dir);

        // Deposit funds for User 1: 1000 Asset 2 (Quote)
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 2, amount: 1000 })
            .unwrap();

        //    // Add order: Buy 50 @ 10 (Cost 500) -> Success
        let result = engine.add_order(0, 1, Side::Buy, OrderType::Limit, 10, 50, 1, 0);
        assert!(result.is_ok(), "Order should be accepted with sufficient funds");
    }

    #[test]
    fn test_add_order_insufficient_funds() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = setup_engine(&temp_dir);

        // Deposit funds for User 1: 500 Asset 2 (Quote)
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 2, amount: 500 }).unwrap();

        // Add order: Buy 100 @ 10 (Cost 1000) -> Fails (Balance 500)
        let result = engine.add_order(0, 1, Side::Buy, OrderType::Limit, 10, 100, 1, 0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, OrderError::InsufficientFunds { .. }));

        match err {
            OrderError::InsufficientFunds { user_id, asset_id, required, avail } => {
                assert_eq!(user_id, 1);
                assert_eq!(asset_id, 2);
                assert_eq!(required, 1000);
                assert_eq!(avail, 500);
            }
            _ => panic!("Expected InsufficientFunds error"),
        }
    }

    #[test]
    fn test_add_order_no_account() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = setup_engine(&temp_dir);

        // User 2 has no account/funds

        // Place Buy Order
        let result = engine.add_order(0, 1, Side::Buy, OrderType::Limit, 100, 10, 2, 0);
        assert!(result.is_err(), "Order should be rejected for user with no funds");

        match result.unwrap_err() {
            OrderError::InsufficientFunds { user_id, asset_id, required, avail } => {
                assert_eq!(user_id, 2);
                assert_eq!(asset_id, 2);
                assert_eq!(required, 1000);
                assert_eq!(avail, 0);
            }
            _ => panic!("Expected InsufficientFunds error"),
        }
    }

    #[test]
    fn test_add_order_invalid_symbol() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = setup_engine(&temp_dir);

        // Add order with invalid symbol_id 999
        let result = engine.add_order(999, 1, Side::Buy, OrderType::Limit, 100, 10, 1, 0);
        let err = result.unwrap_err();
        assert!(matches!(err, OrderError::InvalidSymbol { symbol_id: 999 }));

        match err {
            OrderError::InvalidSymbol { symbol_id } => {
                assert_eq!(symbol_id, 999);
            }
            _ => panic!("Expected InvalidSymbol error"),
        }
    }
}
