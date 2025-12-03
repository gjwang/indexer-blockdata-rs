use reqwest::Client;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let gateway_url = "http://localhost:8082";

    println!("=== Deposit/Withdraw Test Client ===\n");

    // Test 1: Deposit from blockchain
    println!("📥 Test 1: Blockchain Deposit");
    let deposit_payload = json!({
        "request_id": "deposit_btc_001",
        "user_id": 1001,
        "asset_id": 1,  // BTC
        "amount": 100000000,  // 1 BTC (in satoshis)
        "chain": "Bitcoin",
        "external_tx_id": "0x1234567890abcdef",
        "confirmations": 6,
        "required_confirmations": 6
    });

    let response = client
        .post(format!("{}/api/v1/deposit", gateway_url))
        .json(&deposit_payload)
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    // Wait a bit for processing
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 2: Another deposit with insufficient confirmations
    println!("📥 Test 2: Blockchain Deposit (Insufficient Confirmations)");
    let deposit_payload2 = json!({
        "request_id": "deposit_eth_002",
        "user_id": 1002,
        "asset_id": 3,  // ETH
        "amount": 5000000000000000000u64,  // 5 ETH (in wei)
        "chain": "Ethereum",
        "external_tx_id": "0xabcdef1234567890",
        "confirmations": 5,
        "required_confirmations": 12
    });

    let response = client
        .post(format!("{}/api/v1/deposit", gateway_url))
        .json(&deposit_payload2)
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 3: Withdrawal
    println!("📤 Test 3: Blockchain Withdrawal");
    let withdraw_payload = json!({
        "request_id": "withdraw_btc_003",
        "user_id": 1001,
        "asset_id": 1,  // BTC
        "amount": 50000000,  // 0.5 BTC
        "chain": "Bitcoin",
        "address": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"
    });

    let response = client
        .post(format!("{}/api/v1/withdraw", gateway_url))
        .json(&withdraw_payload)
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Test 4: Duplicate deposit (idempotency test)
    println!("📥 Test 4: Duplicate Deposit (Idempotency Test)");
    let response = client
        .post(format!("{}/api/v1/deposit", gateway_url))
        .json(&deposit_payload)  // Same as Test 1
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    // Test 5: Health check
    println!("🏥 Test 5: Health Check");
    let response = client
        .get(format!("{}/health", gateway_url))
        .send()
        .await?;

    println!("Response: {}", response.status());
    println!("Body: {}\n", response.text().await?);

    println!("=== All tests completed ===");

    Ok(())
}
