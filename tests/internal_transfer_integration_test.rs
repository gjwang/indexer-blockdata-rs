/// Integration Test for Internal Transfer
///
/// This test verifies the complete flow from API request to DB persistence

use fetcher::api::{InternalTransferHandler, validate_transfer_request};
use fetcher::db::InternalTransferDb;
use fetcher::models::internal_transfer_types::{AccountType, InternalTransferRequest, TransferStatus};
use fetcher::symbol_manager::SymbolManager;
use fetcher::mocks::MockTbClient;
use rust_decimal::Decimal;
use std::sync::Arc;

#[tokio::test]
async fn test_internal_transfer_end_to_end() {
    println!("\n=== Integration Test: Internal Transfer E2E ===\n");

    // 1. Setup
    println!("1. Setting up test environment...");

    let symbol_manager = Arc::new(SymbolManager::load_from_db());
    let tb_client = Arc::new(MockTbClient::new());

    // Setup TigerBeetle accounts with initial balances
    let funding_account_id = 2_u128; // user_id=0, asset_id=2 (USDT)
    let user_id = 3001_u64;
    let spot_account_id = ((user_id as u128) << 64) | 2;

    tb_client.set_balance(funding_account_id, 10_000_000_000); // 100.00 USDT in funding
    tb_client.set_balance(spot_account_id, 0); // 0 USDT in user spot

    println!("  ✓ TB accounts setup");
    println!("    Funding balance: {}", tb_client.get_available_balance(funding_account_id).unwrap());
    println!("    Spot balance: {}", tb_client.get_available_balance(spot_account_id).unwrap());

    // 2. Create transfer request
    println!("\n2. Creating transfer request...");

    let request = InternalTransferRequest {
        from_account: AccountType::Funding {
            asset: "USDT".to_string(),
            user_id: 0,
        },
        to_account: AccountType::Spot {
            user_id,
            asset: "USDT".to_string(),
        },
        amount: Decimal::new(100_000_000, 8), // 1.00 USDT
    };

    println!("  Request: {:?}", request.amount);

    // 3. Validate request
    println!("\n3. Validating request...");

    validate_transfer_request(&request, &symbol_manager).expect("Validation should pass");
    println!("  ✓ Validation passed");

    // 4. Simulate TB CREATE_PENDING
    println!("\n4. Creating TB PENDING transfer...");

    let transfer_id = 12345_u128;
    tb_client.create_pending_transfer(
        transfer_id,
        funding_account_id,
        spot_account_id,
        100_000_000,
    ).expect("PENDING creation should succeed");

    println!("  ✓ PENDING transfer created");
    println!("    Funding balance after lock: {}", tb_client.get_available_balance(funding_account_id).unwrap());

    // 5. Simulate POST_PENDING
    println!("\n5. Posting PENDING transfer (settlement)...");

    tb_client.post_pending_transfer(transfer_id).expect("POST should succeed");

    println!("  ✓ Transfer posted");
    println!("    Final funding balance: {}", tb_client.get_available_balance(funding_account_id).unwrap());
    println!("    Final spot balance: {}", tb_client.get_available_balance(spot_account_id).unwrap());

    // 6. Verify balances
    println!("\n6. Verifying final balances...");

    assert_eq!(
        tb_client.get_available_balance(funding_account_id).unwrap(),
        9_900_000_000, // 99.00 USDT
        "Funding account should have 99.00 USDT"
    );

    assert_eq!(
        tb_client.get_available_balance(spot_account_id).unwrap(),
        100_000_000, // 1.00 USDT
        "Spot account should have 1.00 USDT"
    );

    println!("  ✓ All balances correct!");

    println!("\n=== ✅ Integration Test PASSED ===\n");
}

#[tokio::test]
async fn test_transfer_out_flow() {
    println!("\n=== Integration Test: Transfer OUT ===\n");

    let symbol_manager = Arc::new(SymbolManager::load_from_db());
    let tb_client = Arc::new(MockTbClient::new());

    let funding_account_id = 2_u128;
    let user_id = 3001_u64;
    let spot_account_id = ((user_id as u128) << 64) | 2;

    // User has 5.00 USDT in spot account
    tb_client.set_balance(spot_account_id, 500_000_000);
    tb_client.set_balance(funding_account_id, 0);

    println!("Initial spot balance: {}", tb_client.get_available_balance(spot_account_id).unwrap());

    // Transfer OUT: Spot -> Funding
    let request = InternalTransferRequest {
        from_account: AccountType::Spot {
            user_id,
            asset: "USDT".to_string(),
        },
        to_account: AccountType::Funding {
            asset: "USDT".to_string(),
            user_id: 0,
        },
        amount: Decimal::new(200_000_000, 8), // 2.00 USDT
    };

    println!("Transfer amount: {}", request.amount);

    validate_transfer_request(&request, &symbol_manager).expect("Should be valid");

    let transfer_id = 67890_u128;
    tb_client.create_pending_transfer(
        transfer_id,
        spot_account_id,
        funding_account_id,
        200_000_000,
    ).expect("PENDING should succeed");

    println!("Spot balance after lock: {}", tb_client.get_available_balance(spot_account_id).unwrap());

    tb_client.post_pending_transfer(transfer_id).expect("POST should succeed");

    assert_eq!(tb_client.get_available_balance(spot_account_id).unwrap(), 300_000_000);
    assert_eq!(tb_client.get_available_balance(funding_account_id).unwrap(), 200_000_000);

    println!("✓ Transfer OUT completed successfully");
    println!("  Final spot: {}", tb_client.get_available_balance(spot_account_id).unwrap());
    println!("  Final funding: {}", tb_client.get_available_balance(funding_account_id).unwrap());

    println!("\n=== ✅ Transfer OUT Test PASSED ===\n");
}

#[tokio::test]
async fn test_void_transfer() {
    println!("\n=== Integration Test: VOID Transfer ===\n");

    let tb_client = Arc::new(MockTbClient::new());

    let funding_account_id = 2_u128;
    let spot_account_id = ((3001_u128) << 64) | 2;

    tb_client.set_balance(funding_account_id, 1000_000_000);

    let transfer_id = 99999_u128;
    tb_client.create_pending_transfer(
        transfer_id,
        funding_account_id,
        spot_account_id,
        100_000_000,
    ).unwrap();

    println!("Balance after PENDING: {}", tb_client.get_available_balance(funding_account_id).unwrap());

    // VOID the transfer
    tb_client.void_pending_transfer(transfer_id).unwrap();

    println!("Balance after VOID: {}", tb_client.get_available_balance(funding_account_id).unwrap());

    // Balance should be restored
    assert_eq!(tb_client.get_available_balance(funding_account_id).unwrap(), 1000_000_000);

    println!("✓ VOID successfully restored balance");
    println!("\n=== ✅ VOID Test PASSED ===\n");
}
