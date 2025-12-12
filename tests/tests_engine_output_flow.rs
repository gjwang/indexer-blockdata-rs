// Integration tests for EngineOutput flow (LEGACY)
// Tests the full pipeline: MatchingEngine -> EngineOutput -> Settlement verification
//
// NOTE: These tests use removed methods (transfer_in_to_trading_account).
// The matching engine no longer manages ledger balances directly.
// Balance management is now done via TigerBeetle/UBSCore.

// Disable compilation until tests are updated
#[cfg(any())]
mod legacy_tests {

use fetcher::engine_output::GENESIS_HASH;
use fetcher::matching_engine_base::MatchingEngine;
use fetcher::models::{OrderType, Side};

/// Test that add_order_and_build_output produces valid EngineOutput
#[test]
fn test_matching_engine_produces_valid_engine_output() {
    let temp_dir = tempfile::tempdir().unwrap();
    let wal_dir = temp_dir.path().join("wal");
    let snap_dir = temp_dir.path().join("snap");

    let mut engine = MatchingEngine::new(&wal_dir, &snap_dir, false).unwrap();

    // Setup symbol and deposit
    engine.register_symbol(0, "BTC_USDT".into(), 1, 2).unwrap(); // BTC/USDT
    engine.transfer_in_to_trading_account(1001, 2, 1_000_000).unwrap(); // User 1: 1M USDT
    engine.transfer_in_to_trading_account(1002, 1, 100).unwrap(); // User 2: 100 BTC

    // Place a buy order using the new method
    let (order_id, output) = engine
        .add_order_and_build_output(
            1, // input_seq
            0, // symbol_id
            1, // order_id
            Side::Buy,
            OrderType::Limit,
            15000,         // price
            10,            // quantity
            1001,          // user_id
            1700000000000, // timestamp
            "test_order_1".into(),
        )
        .unwrap();

    // Verify output structure
    assert_eq!(order_id, 1);
    assert_eq!(output.output_seq, 1);
    assert_eq!(output.prev_hash, GENESIS_HASH);

    // Verify chain integrity
    assert!(output.verify());
    assert!(output.verify_prev_hash(GENESIS_HASH));

    // Verify input is captured
    assert_eq!(output.input.input_seq, 1);
    assert!(output.input.verify_crc());

    // Verify order update exists
    assert!(output.order_update.is_some());
    let order_update = output.order_update.as_ref().unwrap();
    assert_eq!(order_update.order_id, 1);
    assert_eq!(order_update.user_id, 1001);

    // Verify balance event (lock)
    assert!(!output.balance_events.is_empty());
    let lock_event = &output.balance_events[0];
    assert_eq!(lock_event.event_type, "lock");
    assert_eq!(lock_event.user_id, 1001);

    // No trades expected (no matching order on other side)
    assert!(output.trades.is_empty());

    println!("✅ EngineOutput verification passed!");
    println!("   Output seq: {}", output.output_seq);
    println!("   Hash: {:016x}", output.hash);
    println!("   Balance events: {}", output.balance_events.len());
}

/// Test chain of EngineOutputs forms valid linked list
#[test]
fn test_engine_output_chain_verification() {
    let temp_dir = tempfile::tempdir().unwrap();
    let wal_dir = temp_dir.path().join("wal");
    let snap_dir = temp_dir.path().join("snap");

    let mut engine = MatchingEngine::new(&wal_dir, &snap_dir, false).unwrap();

    // Setup
    engine.register_symbol(0, "BTC_USDT".into(), 1, 2).unwrap();
    engine.transfer_in_to_trading_account(1001, 2, 1_000_000).unwrap();
    engine.transfer_in_to_trading_account(1002, 1, 100).unwrap();

    // Place first order
    let (_, output1) = engine
        .add_order_and_build_output(
            1,
            0,
            1,
            Side::Buy,
            OrderType::Limit,
            15000,
            10,
            1001,
            1700000000000,
            "order_1".into(),
        )
        .unwrap();

    // Verify first output
    assert_eq!(output1.output_seq, 1);
    assert_eq!(output1.prev_hash, GENESIS_HASH);
    assert!(output1.verify());

    // Place second order (creates a match!)
    let (_, output2) = engine
        .add_order_and_build_output(
            2,
            0,
            2,
            Side::Sell,
            OrderType::Limit,
            15000,
            10,
            1002,
            1700000000001,
            "order_2".into(),
        )
        .unwrap();

    // Verify second output chains to first
    assert_eq!(output2.output_seq, 2);
    assert_eq!(output2.prev_hash, output1.hash); // Chain linkage!
    assert!(output2.verify());
    assert!(output2.verify_chain(&output1));

    // Second order should have trades (matching with first)
    assert!(!output2.trades.is_empty());
    let trade = &output2.trades[0];
    assert_eq!(trade.buyer_user_id, 1001);
    assert_eq!(trade.seller_user_id, 1002);
    assert_eq!(trade.price, 15000);
    assert_eq!(trade.quantity, 10);

    println!("✅ Chain verification passed!");
    println!("   Output1 hash: {:016x}", output1.hash);
    println!("   Output2 prev_hash: {:016x}", output2.prev_hash);
    println!("   Output2 hash: {:016x}", output2.hash);
    println!("   Trades: {}", output2.trades.len());
    println!("   Balance events: {}", output2.balance_events.len());
}

/// Test that settlement service chain verification logic works
#[test]
fn test_settlement_chain_verification_logic() {
    use fetcher::engine_output::{
        BalanceEvent as EOBalanceEvent, EngineOutput, EngineOutputBuilder, InputBundle, InputData,
        OrderUpdate as EOOrderUpdate, PlaceOrderInput,
    };

    // Simulate settlement service state
    let mut last_hash = GENESIS_HASH;
    let mut last_seq = 0u64;

    // Create first output
    let input1 = InputBundle::new(
        1,
        InputData::PlaceOrder(PlaceOrderInput {
            order_id: 1,
            user_id: 1001,
            symbol_id: 0,
            side: 1,
            order_type: 1,
            price: 15000,
            quantity: 10,
            cid: "test1".into(),
            created_at: 1700000000000,
        }),
    );

    let output1 = EngineOutput::new(
        1,
        last_hash,
        input1,
        Some(EOOrderUpdate {
            order_id: 1,
            user_id: 1001,
            status: 1, // New
            filled_qty: 0,
            remaining_qty: 10,
            avg_price: 0,
            updated_at: 1700000000000,
        }),
        vec![],
        vec![EOBalanceEvent {
            user_id: 1001,
            asset_id: 2,
            seq: 1,
            delta_avail: -150000,
            delta_frozen: 150000,
            avail: 850000,
            frozen: 150000,
            event_type: "lock".into(),
            ref_id: 1,
        }],
    );

    // Settlement service verification
    assert!(output1.verify_prev_hash(last_hash), "prev_hash should match");
    assert!(output1.verify(), "hash should be valid");
    assert_eq!(output1.output_seq, last_seq + 1, "sequence should be continuous");

    // Update settlement state
    last_hash = output1.hash;
    last_seq = output1.output_seq;

    // Create second output (chained)
    let input2 = InputBundle::new(
        2,
        InputData::PlaceOrder(PlaceOrderInput {
            order_id: 2,
            user_id: 1002,
            symbol_id: 0,
            side: 2,
            order_type: 1,
            price: 15000,
            quantity: 10,
            cid: "test2".into(),
            created_at: 1700000000001,
        }),
    );

    let output2 = EngineOutput::new(
        2,
        last_hash, // Chains to output1
        input2,
        Some(EOOrderUpdate {
            order_id: 2,
            user_id: 1002,
            status: 3, // Filled
            filled_qty: 10,
            remaining_qty: 0,
            avg_price: 15000,
            updated_at: 1700000000001,
        }),
        vec![],
        vec![],
    );

    // Settlement service verification for second output
    assert!(output2.verify_prev_hash(last_hash), "prev_hash should chain to output1");
    assert!(output2.verify(), "hash should be valid");
    assert_eq!(output2.output_seq, last_seq + 1, "sequence should be continuous");

    // Test broken chain detection
    let bad_output = EngineOutput::new(
        3,
        999999, // Wrong prev_hash!
        InputBundle::new(
            3,
            InputData::PlaceOrder(PlaceOrderInput {
                order_id: 3,
                user_id: 1001,
                symbol_id: 0,
                side: 1,
                order_type: 1,
                price: 15000,
                quantity: 5,
                cid: "test3".into(),
                created_at: 1700000000002,
            }),
        ),
        None,
        vec![],
        vec![],
    );

    assert!(!bad_output.verify_prev_hash(output2.hash), "should detect broken chain");

    println!("✅ Settlement chain verification logic passed!");
}

} // mod legacy_tests
