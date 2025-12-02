use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use reqwest::Client;

use fetcher::fast_ulid::FastUlidHalfGen;

#[tokio::main]
async fn main() {
    let client = Client::new();
    let api_url = "http://localhost:3001/api/orders";

    // Simulate raw input symbols
    let symbols: Vec<&str> = vec!["BTC_USDT", "ETH_USDT"]; //FIXME:

    println!(">>> Starting Order Client (Concurrent)");
    println!(">>> Target: {}", api_url);

    let cid_gen = Arc::new(Mutex::new(FastUlidHalfGen::new()));
    let counter = Arc::new(AtomicU64::new(0));
    let total_count: u64 = 1000000;
    
    let concurrency = 500;
    let interval_ms = 0;

    let mut handles = Vec::new();

    for _ in 0..concurrency {
        let client = client.clone();
        let cid_gen = cid_gen.clone();
        let counter = counter.clone();
        let symbols = symbols.clone();

        let handle = tokio::spawn(async move {
            loop {
                let i = counter.fetch_add(1, Ordering::Relaxed);
                if i >= total_count {
                    break;
                }

                // pick a symbol (round‑robin)
                let raw_symbol = symbols[i as usize % symbols.len()];

                // deterministic price & quantity (same for both sides)
                let price_step = ((i / 2) % 100) as f64;
                let price = format!("{:.2}", 50000.0 + price_step);

                // ---- SELL order ------------------------------------------------
                // 1. Send three small SELL orders
                for _ in 0..3 {
                    let quantity = ((i + 1) as f64).to_string();
                    
                    let sell_cid = {
                        let mut gen = cid_gen.lock().unwrap();
                        format!("{:012}", gen.generate())
                    };

                    let sell_payload = serde_json::json!({
                        "cid": sell_cid,
                        "symbol": raw_symbol,
                        "side": "Sell",
                        "price": price,
                        "quantity": quantity,
                        "order_type": "Limit"
                    });

                    // send SELL and handle response
                    match client.post(api_url).json(&sell_payload).send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            if !status.is_success() {
                                let err_body = resp
                                    .text()
                                    .await
                                    .unwrap_or_else(|_| "<failed to read body>".into());
                                eprintln!("SELL request error: {} - {}", status, err_body);
                            } else {
                                println!("SELL request succeeded: {}", status);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to send SELL request: {:?}", e);
                        }
                    }

                    // Print brief info of the SELL order
                    println!(
                        "SELL order {}: symbol={}, price={}, qty={}",
                        sell_cid, raw_symbol, price, quantity
                    );
                }

                // ---- BUY order -------------------------------------------------
                // Generate a quantity that can match multiple opposite orders (1.0 .. 5.0)
                let quantity = "6.0";

                let buy_cid = {
                    let mut gen = cid_gen.lock().unwrap();
                    format!("{:012}", gen.generate())
                };

                let buy_payload = serde_json::json!({
                    "cid": buy_cid,
                    "symbol": raw_symbol,
                    "side": "Buy",
                    "price": price,
                    "quantity": quantity,
                    "order_type": "Limit"
                });

                // send BUY and handle response
                match client.post(api_url).json(&buy_payload).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        if !status.is_success() {
                            let err_body = resp
                                .text()
                                .await
                                .unwrap_or_else(|_| "<failed to read body>".into());
                            eprintln!("BUY request error: {} - {}", status, err_body);
                        } else {
                            println!("BUY request succeeded: {}", status);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to send BUY request: {:?}", e);
                    }
                }

                // Print brief info of the BUY order
                println!(
                    "BUY order {}: symbol={}, price={}, qty={}",
                    buy_cid, raw_symbol, price, quantity
                );

                // optional throttle (removed as per request for speed, but keeping structure if needed)
                time::sleep(Duration::from_millis(interval_ms)).await;
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }
    println!(">>> All requests completed.");
}
