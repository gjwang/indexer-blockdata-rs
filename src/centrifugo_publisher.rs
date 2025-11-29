use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Centrifugo publisher for sending real-time updates to users
pub struct CentrifugoPublisher {
    client: Client,
    api_url: String,
    api_key: String,
}

impl CentrifugoPublisher {
    /// Create a new Centrifugo publisher with optimized HTTP client
    pub fn new(api_url: String, api_key: String) -> Self {
        let client = Client::builder()
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            api_url,
            api_key,
        }
    }

    /// Publish balance update to a specific user's private channel
    pub async fn publish_balance_update(
        &self,
        user_id: &str,
        balance_data: BalanceUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = format!("user:{}#balance", user_id);
        self.publish(&channel, &balance_data).await
    }

    /// Publish order update to a specific user's private channel
    pub async fn publish_order_update(
        &self,
        user_id: &str,
        order_data: OrderUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = format!("user:{}#orders", user_id);
        self.publish(&channel, &order_data).await
    }

    /// Publish position update to a specific user's private channel
    pub async fn publish_position_update(
        &self,
        user_id: &str,
        position_data: PositionUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = format!("user:{}#positions", user_id);
        self.publish(&channel, &position_data).await
    }

    /// Generic publish method for any channel and data
    pub async fn publish<T: Serialize>(
        &self,
        channel: &str,
        data: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = json!({
            "channel": channel,
            "data": data
        });

        let response = self
            .client
            .post(&format!("{}/publish", self.api_url))
            .header("X-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Centrifugo publish failed: {} - {}", status, error_text).into());
        }

        Ok(())
    }

    /// Publish to multiple users at once (batch)
    pub async fn publish_to_users<T: Serialize>(
        &self,
        user_ids: &[&str],
        channel_suffix: &str,
        data: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let commands: Vec<_> = user_ids
            .iter()
            .map(|user_id| {
                json!({
                    "publish": {
                        "channel": format!("user:{}#{}", user_id, channel_suffix),
                        "data": data
                    }
                })
            })
            .collect();

        let body = json!({
            "commands": commands
        });

        let response = self
            .client
            .post(&format!("{}/batch", self.api_url))
            .header("X-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Centrifugo batch publish failed: {} - {}", status, error_text).into());
        }

        Ok(())
    }
}

/// Balance update event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub asset: String,
    pub available: f64,
    pub locked: f64,
    pub total: f64,
    pub timestamp: i64,
}

/// Order update event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub order_id: String,
    pub symbol: String,
    pub side: String,        // "buy" or "sell"
    pub order_type: String,  // "limit", "market", etc.
    pub status: String,      // "new", "filled", "cancelled", etc.
    pub price: f64,
    pub quantity: f64,
    pub filled_quantity: f64,
    pub remaining_quantity: f64,
    pub timestamp: i64,
}

/// Position update event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub symbol: String,
    pub side: String,           // "long" or "short"
    pub quantity: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub liquidation_price: f64,
    pub unrealized_pnl: f64,
    pub leverage: f64,
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_publisher_creation() {
        let publisher = CentrifugoPublisher::new(
            "http://localhost:8000/api".to_string(),
            "test_key".to_string(),
        );
        assert_eq!(publisher.api_url, "http://localhost:8000/api");
    }

    #[test]
    fn test_balance_update_serialization() {
        let balance = BalanceUpdate {
            asset: "BTC".to_string(),
            available: 1.5,
            locked: 0.2,
            total: 1.7,
            timestamp: 1732900000,
        };

        let json = serde_json::to_string(&balance).unwrap();
        assert!(json.contains("BTC"));
        assert!(json.contains("1.5"));
    }
}
