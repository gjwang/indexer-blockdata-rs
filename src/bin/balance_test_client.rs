use reqwest::Client;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let gateway_url = "http://localhost:8083";

    println!("=== Transfer Server Test Client ===\n");
    println!("Note: Requests will take ~5s to simulate settlement\n");
    println!("All transfers are internal: funding_account <-> trading_account");
    println!("Time window: 60 seconds");
    println!("Using ULID for unique request_id\n");

    // Test 1: Transfer In to user's trading account
    println!("📥 Test 1: Transfer In (funding_account → user 1001)");
    let request_id = ulid::Ulid::new().to_string();
    let transfer_in_payload = json!({
        "request_id": request_id,
        "user_id": 1001,
        "asset_id": 1,  // BTC
        "amount": 100000000  // 1 BTC (in satoshis)
    });

    let response = client
        .post(format!("{}/api/v1/transfer_in", gateway_url))
        .json(&transfer_in_payload)
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 2: Another transfer in to different user
    println!("📥 Test 2: Transfer In (funding_account → user 1002)");
    let request_id = ulid::Ulid::new().to_string();
    let transfer_in_payload2 = json!({
        "request_id": request_id,
        "user_id": 1002,
        "asset_id": 2,  // USDT
        "amount": 10000000000u64  // 10,000 USDT
    });

    let response = client
        .post(format!("{}/api/v1/transfer_in", gateway_url))
        .json(&transfer_in_payload2)
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 3: Transfer Out from user's trading account
    println!("📤 Test 3: Transfer Out (user 1001 → funding_account)");
    let request_id = ulid::Ulid::new().to_string();
    let transfer_out_payload = json!({
        "request_id": request_id,
        "user_id": 1001,
        "asset_id": 1,  // BTC
        "amount": 50000000  // 0.5 BTC
    });

    let response = client
        .post(format!("{}/api/v1/transfer_out", gateway_url))
        .json(&transfer_out_payload)
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 4: Duplicate transfer in (idempotency test) - reuse same request_id
    println!("📥 Test 4: Duplicate Transfer In (Idempotency Test)");
    let duplicate_request_id = ulid::Ulid::new().to_string();
    let dup_payload = json!({
        "request_id": duplicate_request_id,
        "user_id": 1001,
        "asset_id": 1,
        "amount": 100000000
    });

    // Send first time
    let response = client
        .post(format!("{}/api/v1/transfer_in", gateway_url))
        .json(&dup_payload)
        .send()
        .await?;
    println!("First request: {}", response.status());

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Send duplicate
    let response = client
        .post(format!("{}/api/v1/transfer_in", gateway_url))
        .json(&dup_payload) // Same request_id
        .send()
        .await?;

    println!("Duplicate request: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 5: Multiple rapid transfer ins (stress test) - each with unique ULID
    println!("📥 Test 5: Rapid Transfer Ins (Stress Test - Unique ULIDs)");
    for i in 0..5 {
        let request_id = ulid::Ulid::new().to_string();
        let payload = json!({
            "request_id": request_id,
            "user_id": 1003,
            "asset_id": 3,  // ETH
            "amount": 1000000000000000000u64  // 1 ETH
        });

        let response = client
            .post(format!("{}/api/v1/transfer_in", gateway_url))
            .json(&payload)
            .send()
            .await?;

        println!(
            "  Transfer In {} ({}): {}",
            i,
            request_id,
            response.status()
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 6: Health check
    println!("🏥 Test 6: Health Check");
    let response = client.get(format!("{}/health", gateway_url)).send().await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    println!("=== All tests completed ===");
    println!("\nNote: Check balance_processor logs to see:");
    println!("  - Transfers between funding_account and trading accounts");
    println!("  - Duplicate detection");
    println!("  - Automatic cleanup of old requests");

    Ok(())
}
