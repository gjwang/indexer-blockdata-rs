use anyhow::Result;
use reqwest::Client;
use serde::Serialize;
use std::time::Duration;

const STARROCKS_URL: &str = "http://localhost:8040/api/settlement/trades/_stream_load";

#[derive(Debug, Serialize)]
pub struct StarRocksTrade {
    pub trade_id: i64,
    pub match_seq: i64,
    pub buy_order_id: i64,
    pub sell_order_id: i64,
    pub buy_user_id: i64,
    pub sell_user_id: i64,
    pub price: i64,
    pub quantity: i64,
    pub trade_date: String,
    pub settled_at: String,
}

impl StarRocksTrade {
    pub fn from_match_exec(data: &crate::ledger::MatchExecData, timestamp_ms: i64) -> Self {
        let datetime = chrono::DateTime::from_timestamp_millis(timestamp_ms)
            .unwrap_or_else(|| chrono::Utc::now());

        Self {
            trade_id: data.trade_id as i64,
            match_seq: data.match_seq as i64,
            buy_order_id: data.buy_order_id as i64,
            sell_order_id: data.sell_order_id as i64,
            buy_user_id: data.buyer_user_id as i64,
            sell_user_id: data.seller_user_id as i64,
            price: data.price as i64,
            quantity: data.quantity as i64,
            trade_date: datetime.format("%Y-%m-%d").to_string(),
            settled_at: datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

pub struct StarRocksClient {
    client: Client,
    url: String,
    label_counter: std::sync::atomic::AtomicU64,
}

impl StarRocksClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            url: STARROCKS_URL.to_string(),
            label_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Load a single trade to StarRocks (fire-and-forget, non-blocking)
    pub async fn load_trade(&self, trade: StarRocksTrade) -> Result<()> {
        self.load_trades(vec![trade]).await
    }

    /// Load multiple trades to StarRocks in a batch
    pub async fn load_trades(&self, trades: Vec<StarRocksTrade>) -> Result<()> {
        if trades.is_empty() {
            return Ok(());
        }

        let label_num = self.label_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let label = format!("settlement_{}", label_num);

        // Convert to JSON Array
        let payload = serde_json::to_string(&trades)?;
        // println!("[DEBUG] StarRocks Payload: {}", payload); // Uncomment for verbose debug

        let response = self.client
            .put(&self.url)
            .basic_auth("root", Some(""))
            .header("label", &label)
            .header("format", "json")
            .header("strip_outer_array", "true")
            .body(payload)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            anyhow::bail!("StarRocks load failed: HTTP {} - {}", status, body);
        }

        let result: serde_json::Value = serde_json::from_str(&body)?;

        if result["Status"] != "Success" {
            anyhow::bail!("StarRocks load failed: {}", result["Message"]);
        }

        Ok(())
    }
}

impl Default for StarRocksClient {
    fn default() -> Self {
        Self::new()
    }
}
