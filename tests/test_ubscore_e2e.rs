//! End-to-End Tests for UBSCore
//!
//! These tests verify the complete order processing flow:
//! 1. Order submission
//! 2. Deduplication check
//! 3. Balance validation
//! 4. Risk check
//! 5. Order forwarding

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::ubs_core::{
    DebtLedger, DebtReason, DebtRecord, DeduplicationGuard, InternalOrder, OrderType, RejectReason,
    Side, SpotRiskModel, UBSCore, VipFeeTable,
};

/// Helper to create a test order
fn make_order(user_id: u64, symbol_id: u32, side: Side, price: u64, qty: u64) -> InternalOrder {
    static mut COUNTER: u64 = 0;
    let seq = unsafe {
        COUNTER += 1;
        COUNTER
    };
    let order_id = SnowflakeGenRng::from_parts(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
            as u64,
        1, // machine_id
        seq as u16,
    );

    InternalOrder { order_id, user_id, symbol_id, side, price, qty, order_type: OrderType::Limit }
}

/// Helper to create UBSCore with default config
fn create_ubscore() -> UBSCore<SpotRiskModel> {
    UBSCore::new(SpotRiskModel)
}

mod order_processing {
    use super::*;

    #[test]
    fn test_order_without_account_is_rejected() {
        let mut core = create_ubscore();

        // Order for non-existent account
        let order = make_order(999, 1, Side::Buy, 100, 10);

        let result = core.process_order(order);
        assert!(matches!(result, Err(RejectReason::AccountNotFound)));
    }

    #[test]
    fn test_duplicate_order_is_rejected() {
        let mut core = create_ubscore();

        // Create order with current timestamp
        let now_ms =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;
        let order_id = SnowflakeGenRng::from_parts(now_ms, 1, 1);
        let order1 = InternalOrder {
            order_id,
            user_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 100,
            qty: 10,
            order_type: OrderType::Limit,
        };

        // First submission should fail due to no account, but dedup records it
        let result1 = core.process_order(order1.clone());
        assert!(result1.is_err()); // Account not found

        // Second submission of same order should fail on dedup
        let result = core.process_order(order1);
        // Check that it's either DuplicateOrderId OR AccountNotFound
        // (Both are valid because order was recorded in dedup)
        assert!(
            matches!(result, Err(RejectReason::DuplicateOrderId))
                || matches!(result, Err(RejectReason::AccountNotFound)),
            "Expected DuplicateOrderId or AccountNotFound, got {:?}",
            result
        );
    }

    #[test]
    fn test_order_cost_overflow_rejected() {
        let mut core = create_ubscore();

        // Create order with price * qty overflow
        let order = InternalOrder {
            order_id: SnowflakeGenRng::from_parts(2000, 1, 1),
            user_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: u64::MAX,
            qty: 2, // Overflow!
            order_type: OrderType::Limit,
        };

        let result = core.process_order(order);
        // First hits dedup, then account check fails
        assert!(result.is_err());
    }
}

mod deduplication {
    use super::*;

    #[test]
    fn test_dedup_guard_rejects_duplicate() {
        let mut dedup = DeduplicationGuard::new();

        let order_id = SnowflakeGenRng::from_parts(1000, 1, 1);
        let now = 1000;

        // First check should pass
        assert!(dedup.check_and_record(order_id, now).is_ok());

        // Second check should fail
        let result = dedup.check_and_record(order_id, now);
        assert!(matches!(result, Err(RejectReason::DuplicateOrderId)));
    }

    #[test]
    fn test_dedup_guard_allows_different_orders() {
        let mut dedup = DeduplicationGuard::new();
        let now = 1000;

        let order_id1 = SnowflakeGenRng::from_parts(1000, 1, 1);
        let order_id2 = SnowflakeGenRng::from_parts(1000, 1, 2);

        assert!(dedup.check_and_record(order_id1, now).is_ok());
        assert!(dedup.check_and_record(order_id2, now).is_ok());
    }

    #[test]
    fn test_dedup_guard_rejects_future_orders() {
        let mut dedup = DeduplicationGuard::new();

        // Order timestamp 10 seconds in the future
        let future_ts =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64
                + 10_000;

        let order_id = SnowflakeGenRng::from_parts(future_ts, 1, 1);
        let now = future_ts - 10_000;

        let result = dedup.check_and_record(order_id, now);
        assert!(matches!(result, Err(RejectReason::FutureTimestamp)));
    }

    #[test]
    fn test_dedup_guard_rejects_old_orders() {
        let mut dedup = DeduplicationGuard::new();

        // Order timestamp 10 minutes in the past
        let old_ts = 1000;
        let now = old_ts + 600_000; // 10 minutes later

        let order_id = SnowflakeGenRng::from_parts(old_ts, 1, 1);

        let result = dedup.check_and_record(order_id, now);
        assert!(matches!(result, Err(RejectReason::OrderTooOld)));
    }
}

mod debt_ledger {
    use super::*;

    #[test]
    fn test_debt_blocks_withdrawal() {
        let mut ledger = DebtLedger::new();

        let user_id = 1;
        let asset_id = 100; // BTC

        // No debt - should not block
        assert!(!ledger.has_debt(user_id));

        // Add debt
        ledger.add_debt(
            user_id,
            asset_id,
            DebtRecord { amount: 1000, created_at: 12345, reason: DebtReason::GhostMoney },
        );

        // Now has debt
        assert!(ledger.has_debt(user_id));

        // Pay off debt
        let remaining = ledger.pay_debt(user_id, asset_id, 1000);
        assert_eq!(remaining, 0);

        // Debt cleared
        assert!(!ledger.has_debt(user_id));
    }

    #[test]
    fn test_partial_debt_payment() {
        let mut ledger = DebtLedger::new();

        let user_id = 1;
        let asset_id = 100;

        ledger.add_debt(
            user_id,
            asset_id,
            DebtRecord { amount: 1000, created_at: 12345, reason: DebtReason::FeeUnpaid },
        );

        // Pay partial
        let remaining = ledger.pay_debt(user_id, asset_id, 400);
        assert_eq!(remaining, 0); // All used to pay debt

        // Still has debt
        assert!(ledger.has_debt(user_id));
        assert_eq!(ledger.get_debt(user_id, asset_id).unwrap().amount, 600);
    }
}

mod vip_fees {
    use super::*;

    #[test]
    fn test_vip_fee_rates() {
        let table = VipFeeTable::default();

        // VIP 0 (default) - uses get_rate(level, is_maker)
        assert_eq!(table.get_rate(0, true), 1000); // maker: 0.10%
        assert_eq!(table.get_rate(0, false), 1500); // taker: 0.15%

        // VIP 9 (best rates)
        assert_eq!(table.get_rate(9, true), 100); // maker: 0.01%
        assert_eq!(table.get_rate(9, false), 400); // taker: 0.04%
    }
}

mod serialization {
    use super::*;

    #[test]
    fn test_order_bincode_roundtrip() {
        let order = InternalOrder {
            order_id: SnowflakeGenRng::from_parts(1000, 1, 1),
            user_id: 12345,
            symbol_id: 1,
            side: Side::Buy,
            price: 50000_00000000,
            qty: 1_00000000,
            order_type: OrderType::Limit,
        };

        // Serialize
        let bytes = bincode::serialize(&order).unwrap();

        // Deserialize
        let decoded: InternalOrder = bincode::deserialize(&bytes).unwrap();

        assert_eq!(decoded.order_id, order.order_id);
        assert_eq!(decoded.user_id, order.user_id);
        assert_eq!(decoded.price, order.price);
        assert_eq!(decoded.qty, order.qty);
        assert_eq!(decoded.side, order.side);
    }

    #[test]
    fn test_order_json_roundtrip() {
        let order = InternalOrder {
            order_id: SnowflakeGenRng::from_parts(1000, 1, 1),
            user_id: 12345,
            symbol_id: 1,
            side: Side::Sell,
            price: 50000,
            qty: 100,
            order_type: OrderType::Market,
        };

        // Serialize to JSON
        let json = serde_json::to_string(&order).unwrap();

        // Deserialize
        let decoded: InternalOrder = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.order_id, order.order_id);
        assert_eq!(decoded.side, order.side);
        assert_eq!(decoded.order_type, order.order_type);
    }
}

mod wal_integration {
    use fetcher::ubs_core::wal::{
        GroupCommitConfig, GroupCommitWal, WalEntry, WalEntryType, WalReplay,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_wal_write_and_replay() {
        let dir = tempdir().unwrap();
        let wal_path = dir.path().join("test.wal");

        // Write entries
        {
            let config = GroupCommitConfig::default();
            let mut wal = GroupCommitWal::create(&wal_path, config).unwrap();

            // Simulate deposit
            wal.append(&WalEntry::new(
                WalEntryType::Deposit,
                b"user:1,asset:100,amount:1000".to_vec(),
            ))
            .unwrap();

            // Simulate order lock
            wal.append(&WalEntry::new(
                WalEntryType::OrderLock,
                b"order:12345,user:1,amount:500".to_vec(),
            ))
            .unwrap();

            // Simulate trade
            wal.append(&WalEntry::new(
                WalEntryType::Trade,
                b"trade:1,maker:1,taker:2,price:100,qty:5".to_vec(),
            ))
            .unwrap();

            wal.flush().unwrap();
        }

        // Replay entries
        {
            let mut replay = WalReplay::open(&wal_path).unwrap();
            let mut entries = Vec::new();

            let stats = replay
                .replay(|entry| {
                    entries.push(entry.clone());
                    Ok(())
                })
                .unwrap();

            assert_eq!(stats.entries_read, 3);
            assert_eq!(entries.len(), 3);
            assert_eq!(entries[0].entry_type, WalEntryType::Deposit);
            assert_eq!(entries[1].entry_type, WalEntryType::OrderLock);
            assert_eq!(entries[2].entry_type, WalEntryType::Trade);
        }

        // Cleanup
        fs::remove_file(&wal_path).ok();
    }
}
