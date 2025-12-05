
#[cfg(test)]
mod tests {
    use crate::matching_engine_base::MatchingEngine;
    use crate::ledger::{LedgerCommand, OrderStatus, OrderUpdate};
    use crate::models::{OrderError, OrderType, Side};
    use tempfile::TempDir;

    fn create_test_engine() -> (MatchingEngine, TempDir, TempDir) {
        let wal_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let engine = MatchingEngine::new(wal_dir.path(), snap_dir.path(), false).unwrap();
        (engine, wal_dir, snap_dir)
    }

    fn fund_user(engine: &mut MatchingEngine, user_id: u64, amount_base: u64, amount_quote: u64) {
        // Fund Base Asset (ID 1)
        if amount_base > 0 {
             engine.ledger.apply(&LedgerCommand::Deposit { user_id, asset: 1, amount: amount_base }).unwrap();
        }
        // Fund Quote Asset (ID 2)
        if amount_quote > 0 {
             engine.ledger.apply(&LedgerCommand::Deposit { user_id, asset: 2, amount: amount_quote }).unwrap();
        }
    }

    #[test]
    fn test_order_full_fill() {
        let (mut engine, _wal, _snap) = create_test_engine();
        engine.register_symbol(1, "BTC_USDT".to_string(), 1, 2).unwrap();

        // 1. Setup: Fund Buyer and Seller
        fund_user(&mut engine, 101, 0, 100_000_000);   // Buyer has 100M USDT
        fund_user(&mut engine, 102, 10_0000_000, 0); // Seller has 0.1 BTC (10m sat)

        // 2. Place Sell Order (Maker)
        let _ = engine.add_order_batch(vec![(1, 1, Side::Sell, OrderType::Limit, 50000, 1000, 102, 100)]);

        // 3. Place Matching Buy Order (Taker) - Full Fill
        let (results, commands) = engine.add_order_batch(vec![(1, 2, Side::Buy, OrderType::Limit, 50000, 1000, 101, 200)]);

        assert!(results[0].is_ok());

        // Check for MatchExecBatch
        let match_batch = commands.iter().find_map(|c| match c {
            LedgerCommand::MatchExecBatch(batch) => Some(batch),
            _ => None
        });

        assert!(match_batch.is_some(), "Should emit MatchExecBatch");
        let batch = match_batch.unwrap();
        assert_eq!(batch.len(), 1);
        let trade = &batch[0];

        // Assert Trade details
        assert_eq!(trade.buy_order_id, 2);
        assert_eq!(trade.sell_order_id, 1);
        assert_eq!(trade.quantity, 1000); // Full fill
    }

    #[test]
    fn test_order_partial_fill() {
        let (mut engine, _wal, _snap) = create_test_engine();
        engine.register_symbol(1, "BTC_USDT".to_string(), 1, 2).unwrap();

        fund_user(&mut engine, 101, 0, 100_000_000);
        fund_user(&mut engine, 102, 10_0000_000, 0);

        // Maker: Sell 1000
        let _ = engine.add_order_batch(vec![(1, 1, Side::Sell, OrderType::Limit, 50000, 1000, 102, 100)]);

        // Taker: Buy 500 (Partial of Maker, Full of Taker)
        let (_res, commands) = engine.add_order_batch(vec![(1, 2, Side::Buy, OrderType::Limit, 50000, 500, 101, 200)]);

        // Verify MatchExecBatch
        let match_batch = commands.iter().find_map(|c| match c {
            LedgerCommand::MatchExecBatch(batch) => Some(batch),
            _ => None
        });

        assert!(match_batch.is_some(), "Should emit MatchExecBatch for partial fill");
        let batch = match_batch.unwrap();
        assert_eq!(batch.len(), 1);
        let trade = &batch[0];

        assert_eq!(trade.quantity, 500);

    }

    #[test]
    fn test_duplicate_order_id() {
        let (mut engine, _wal, _snap) = create_test_engine();
        engine.register_symbol(1, "BTC_USDT".to_string(), 1, 2).unwrap();
        fund_user(&mut engine, 101, 0, 100_000);

        engine.add_order_batch(vec![(1, 10, Side::Buy, OrderType::Limit, 100, 10, 101, 100)]);

        // Add same order ID again
        let (results, _) = engine.add_order_batch(vec![(1, 10, Side::Buy, OrderType::Limit, 100, 10, 101, 100)]);

        // Should ERROR immediately (duplicate check is in pre-check? Wait. batch removes check?)
        // In add_order_batch logic, it processes valid orders.
        // Shadow mode processes order logic.
        // `book.add_order` returns Err(DuplicateOrderId) if ID exists.

        // Wait, does `process_order_logic` check duplicates?
        // `book.add_order` calls `book.active_order_ids.contains`.

        assert!(results[0].is_err());
        match &results[0] {
             Err(OrderError::DuplicateOrderId { .. }) => {}, // Good
             Err(OrderError::Other(msg)) if msg.contains("Duplicate") => {},
             Ok(_) => panic!("Duplicate order should fail"),
             _ => panic!("Expected DuplicateOrderId error, got {:?}", results[0]),
        }
    }
}
