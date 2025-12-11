// Demo program showing the full Internal Transfer flow
// This demonstrates:
// 1. Setup (DB, TB mock, handlers)
// 2. Create transfer request
// 3. Check status
// 4. Settlement processing
// 5. Recovery scanning

use anyhow::Result;
use fetcher::api::{InternalTransferHandler, InternalTransferQuery, InternalTransferSettlement};
use fetcher::db::InternalTransferDb;
use fetcher::mocks::tigerbeetle_mock::MockTbClient;
use fetcher::models::internal_transfer_types::{AccountType, InternalTransferRequest};
use fetcher::symbol_manager::SymbolManager;
use rust_decimal::Decimal;
use scylla::{Session, SessionBuilder};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    println!("===========================================");
    println!("Internal Transfer Demo (MVP)");
    println!("===========================================\n");

    // Step 1: Setup
    println!("Step 1: Setting up components...");

    // TODO: Real DB connection
    // For now, this is a compilation demo showing the API
    println!("  ℹ️  Note: This demo shows the API structure");
    println!("  ℹ️  Full execution requires ScyllaDB setup\n");

    // Create mock TigerBeetle client
    let tb_client = Arc::new(MockTbClient::new());

    // Setup funding account with initial balance
    let funding_account_id = 2u128; // asset_id = 2 (USDT)
    tb_client.set_balance(funding_account_id, 1_000_000_000_000); // 10000.00 USDT
    println!("  ✅ Mock TigerBeetle initialized");
    println!("     Funding account balance: 10000.00 USDT\n");

    // In a real scenario:
    /*
    let session = Arc::new(
        SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .build()
            .await?
    );
    let db = Arc::new(InternalTransferDb::new(session.clone()));
    let symbol_manager = Arc::new(SymbolManager::load_from_db(&session).await?);

    // Create handlers
    let handler = InternalTransferHandler::new(
        db.clone(),
        symbol_manager,
        tb_client.clone(),
    );
    let query = InternalTransferQuery::new(db.clone());
    let settlement = Arc::new(InternalTransferSettlement::new(db.clone(), tb_client.clone()));
    */

    println!("Step 2: Transfer Flow Demonstration\n");
    println!("  Scenario: User 3001 transfers 100.00 USDT from Funding to Spot\n");

    // Step 2: Create transfer request
    let request = InternalTransferRequest {
        from_account: AccountType::Funding {
            asset: "USDT".to_string(),
            user_id: 0,
        },
        to_account: AccountType::Spot {
            user_id: 3001,
            asset: "USDT".to_string(),
        },
        amount: Decimal::new(10_000_000_000, 8), // 100.00 USDT
    };

    println!("  Request created:");
    println!("    From: Funding (USDT)");
    println!("    To: Spot (user_id=3001, USDT)");
    println!("    Amount: 100.00 USDT\n");

    // With real DB, this would execute:
    /*
    println!("  Processing...");
    let response = handler.handle_transfer(request).await?;

    if response.status == 0 {
        println!("  ✅ Transfer created successfully!");
        if let Some(data) = response.data {
            println!("     Request ID: {}", data.request_id);
            println!("     Status: {:?}", data.status);
            println!("     Created at: {}", data.created_at);
        }
    } else {
        println!("  ❌ Transfer failed: {}", response.msg);
    }
    */

    println!("  Expected Flow:");
    println!("    1. ✅ Validate request (asset match, precision)");
    println!("    2. ✅ Generate unique request_id (Snowflake)");
    println!("    3. ✅ Insert DB record (status: 'requesting')");
    println!("    4. ✅ Check TB funding balance (≥ 100.00)");
    println!("    5. ✅ Create TB PENDING (lock 100.00 USDT)");
    println!("    6. ✅ Update DB (status: 'pending')");
    println!("    7. ⏭️  Send to UBSCore (future)");
    println!("    8. ✅ Return response\n");

    // Step 3: Settlement Processing
    println!("Step 3: Settlement Processing\n");

    // With real system:
    /*
    // Simulating UBSCore confirmation
    let request_id = 1234567890_i64;
    println!("  UBSCore confirmed transfer {}", request_id);

    settlement.process_confirmation(request_id).await?;
    println!("  ✅ Settlement POST_PENDING completed");
    println!("  ✅ DB updated to 'success'\n");
    */

    println!("  Expected Flow:");
    println!("    1. ✅ Settlement receives Kafka event from UBSCore");
    println!("    2. ✅ Settlement calls TB POST_PENDING");
    println!("    3. ✅ Funds transferred to user spot account");
    println!("    4. ✅ DB updated (status: 'success')\n");

    // Step 4: Query Status
    println!("Step 4: Query Transfer Status\n");

    // With real system:
    /*
    let status_response = query.get_transfer_status("1234567890").await?;
    if status_response.status == 0 {
        if let Some(data) = status_response.data {
            println!("  Transfer Status:");
            println!("    Request ID: {}", data.request_id);
            println!("    Status: {:?}", data.status);
            println!("    Amount: {}", data.amount);
        }
    }
    */

    println!("  API: GET /api/v1/user/internal_transfer/{{id}}");
    println!("  Returns:");
    println!("    - request_id");
    println!("    - from_account");
    println!("    - to_account");
    println!("    - amount");
    println!("    - status (requesting/pending/success/failed)");
    println!("    - created_at\n");

    // Step 5: Recovery Scanner
    println!("Step 5: Crash Recovery Demonstration\n");

    println!("  Scenario: Gateway crashed after TB lock, before DB update\n");
    println!("  Recovery Scanner runs every 5s:");
    println!("    1. ✅ Query DB for 'requesting' transfers");
    println!("    2. ✅ Check TB status for each");
    println!("    3. ✅ If TB=PENDING, update DB to 'pending'");
    println!("    4. ✅ If TB=POSTED, update DB to 'success'");
    println!("    5. ✅ If TB=VOIDED, update DB to 'failed'");
    println!("    6. ⚠️  Alert if pending > 30min");
    println!("    7. 🚨 Critical alert if pending > 2h\n");

    // Step 6: Balance Verification
    println!("Step 6: Final Balance Check\n");

    let funding_balance = tb_client.get_available_balance(funding_account_id)?;
    println!("  Funding account:");
    println!("    Before: 10000.00 USDT");
    println!("    After: {}.{:08} USDT", funding_balance / 100_000_000, funding_balance % 100_000_000);

    let spot_account_id = ((3001u128) << 64) | 2; // user_id=3001, asset_id=2
    let spot_balance = tb_client.get_available_balance(spot_account_id)?;
    println!("  Spot account (user_id=3001):");
    println!("    Before: 0.00 USDT");
    println!("    After: {}.{:08} USDT\n", spot_balance / 100_000_000, spot_balance % 100_000_000);

    println!("===========================================");
    println!("Demo Complete!");
    println!("===========================================\n");
    println!("Next Steps:");
    println!("  1. Set up ScyllaDB: docker run -d -p 9042:9042 scylladb/scylla");
    println!("  2. Apply schema: cqlsh -f schema/internal_transfer.cql");
    println!("  3. Run full integration test");
    println!("  4. Deploy to production\n");

    Ok(())
}
