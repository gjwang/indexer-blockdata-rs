use reqwest::Client;
use std::time::Duration;
use tokio::time;

#[tokio::main]
async fn main() {
    let client = Client::new();
    let api_url = "http://localhost:3001/api/orders";

    // Simulate raw input symbols
    let symbols: Vec<&str> = vec!["BTC_USDT", "ETH_USDT"];

    println!(">>> Starting Order Client");
    println!(">>> Target: {}", api_url);

    let count: u64 = 1000000;
    let interval_ms = 100;

    for i in 0..count {
        let raw_symbol = symbols[i as usize % symbols.len()];
        let raw_side = if i % 2 == 0 { "Buy" } else { "Sell" };
        let raw_type = "Limit";
        let price = 50000 + (i % 100);
        let quantity = 1 + (i % 5);


        // Generate a cid. In real app, this might be UUID or similar.
        // We use a simple counter based ID for demo, but ensure it meets validation (20-32 chars).
        // "clientorder" is 11 chars. We need 9 more.
        // i is u64.
        let cid = format!("clientorder{:010}", i); // 11 + 10 = 21 chars.

        let payload = serde_json::json!({
            "cid": cid,
            "symbol": raw_symbol,
            "side": raw_side,
            "price": price,
            "quantity": quantity,
            "order_type": raw_type
        });

        match client.post(api_url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let resp_json = resp.json::<serde_json::Value>().await.unwrap_or_default();
                    println!("Sent order {}: Success {}", i, resp_json);
                } else {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    eprintln!("Failed to send order {}: {} - {}", i, status, text);
                }
            }
            Err(e) => {
                eprintln!("Error sending request: {}", e);
            }
        }

        time::sleep(Duration::from_millis(interval_ms)).await;
    }
}
