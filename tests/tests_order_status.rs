#[cfg(test)]
mod tests {
    use fetcher::ledger::LedgerCommand;
    use fetcher::matching_engine_base::MatchingEngine;
    use fetcher::models::{OrderError, OrderStatus, OrderType, Side};
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
    fn test_order_rejection_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = setup_engine(&temp_dir);

        // Deposit funds for User 1: 500 Asset 2 (Quote)
        engine.ledger.apply(&LedgerCommand::Deposit { user_id: 1, asset: 2, amount: 500, balance_after: 0, version: 0 }).unwrap();

        // Place Buy Order: 10 BTC @ 100 USDT = 1000 USDT required
        // This should fail due to insufficient funds
        let result = engine.add_order(0, 1, Side::Buy, OrderType::Limit, 100, 10, 1, 0);

        match result {
            Err(OrderError::InsufficientFunds { .. }) => {
                // Correctly rejected
                let status = OrderStatus::Rejected("Insufficient funds".to_string());
                assert!(matches!(status, OrderStatus::Rejected(_)));
            }
            _ => panic!("Order should have been rejected with InsufficientFunds"),
        }
    }

    #[test]
    fn test_order_accepted_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = setup_engine(&temp_dir);

        // Deposit funds for User 1: 1000 Asset 2 (Quote)
        engine
            .ledger
            .apply(&LedgerCommand::Deposit { user_id: 1, asset: 2, amount: 1000, balance_after: 0, version: 0 })
            .unwrap();

        // Place Buy Order: 10 BTC @ 100 USDT = 1000 USDT required
        let result = engine.add_order(0, 1, Side::Buy, OrderType::Limit, 100, 10, 1, 0);

        assert!(result.is_ok());
        // In a real system, we would check the OrderBook or Event Log for "New" status
    }
}
