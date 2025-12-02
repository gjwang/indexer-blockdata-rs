use std::time::Duration;

use reqwest::Client;
use tokio::time;

#[tokio::main]
async fn main() {
    let client = Client::new();
    let api_url = "http://localhost:3001/api/orders";

    // Simulate raw input symbols
    let symbols: Vec<&str> = vec!["BTC_USDT", "ETH_USDT"]; //FIXME:

    println!(">>> Starting Order Client");
    println!(">>> Target: {}", api_url);

    let count: u64 = 1000000;
    let interval_ms = 100;

    for i in 0..count {
        // pick a symbol (round‑robin)
        let raw_symbol = symbols[i as usize % symbols.len()];

        // deterministic price & quantity (same for both sides)
        let price_step = ((i / 2) % 100) as f64;
        let price = format!("{:.2}", 50000.0 + price_step);


        // ---- SELL order ------------------------------------------------
        // 1. Send three small SELL orders
        for i in 0..3 {
            let quantity = (i as f64).to_string();
            let sell_cid = format!("clientorder{:010}", i * 2 + 1);
            let sell_payload = serde_json::json!({
                "cid": sell_cid,
                "symbol": raw_symbol,
                "side": "Sell",
                "price": price,
                "quantity": quantity,
                "order_type": "Limit"
            });

            // send SELL
            let _ = client
                .post(api_url)
                .json(&sell_payload)
                .send()
                .await;

            // Print brief info of the SELL order
            println!("SELL order {}: symbol={}, price={}, qty={}", sell_cid, raw_symbol, price, quantity);
        }

        // ---- BUY order -------------------------------------------------
        // Generate a quantity that can match multiple opposite orders (1.0 .. 5.0)
        let quantity = "6.0";

        let buy_cid = format!("clientorder{:010}", i * 2);
        let buy_payload = serde_json::json!({
            "cid": buy_cid,
            "symbol": raw_symbol,
            "side": "Buy",
            "price": price,
            "quantity": quantity,
            "order_type": "Limit"
        });

        // send BUY
        let _ = client
            .post(api_url)
            .json(&buy_payload)
            .send()
            .await;

        // Print brief info of the BUY order
        println!("BUY order {}: symbol={}, price={}, qty={}", buy_cid, raw_symbol, price, quantity);

        // optional throttle
        time::sleep(Duration::from_millis(interval_ms)).await;
    }
}
