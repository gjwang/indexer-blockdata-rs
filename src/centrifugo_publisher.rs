use crate::models::{BalanceUpdate, OrderUpdate, PositionUpdate, UserUpdate};
use reqwest::Client;
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
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client, api_url, api_key }
    }

    /// Publish user update to the user's private channel
    pub async fn publish_user_update<T: serde::Serialize>(
        &self,
        user_id: &str,
        update: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = format!("user:{}", user_id);
        self.publish(&channel, update).await
    }

    /// Publish raw JSON string to the user's private channel
    /// This avoids deserializing and re-serializing the payload
    /// Helper method to send raw JSON request to Centrifugo
    async fn send_raw_request(
        &self,
        endpoint: &str,
        body: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.api_url, endpoint);

        let response = self
            .client
            .post(&url)
            .header("X-API-Key", &self.api_key)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(format!("Centrifugo {} error: {}", endpoint, error_text).into());
        }

        Ok(())
    }

    /// Helper method to send JSON request to Centrifugo (wraps send_raw_request)
    async fn send_request(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body_str = serde_json::to_string(body)?;
        self.send_raw_request(endpoint, body_str).await
    }

    /// Publish raw JSON string to the user's private channel
    /// This avoids deserializing and re-serializing the payload
    pub async fn publish_raw_json(
        &self,
        user_id: &str,
        raw_json: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = format!("user:{}", user_id);

        // We need to construct the body manually to avoid escaping the raw_json string
        // The raw_json is already a valid JSON string, so we inject it directly into the data field
        let body_str = format!(r#"{{"channel":"{}","data":{}}}"#, channel, raw_json);

        self.send_raw_request("publish", body_str).await
    }

    /// Publish balance update to a specific user's private channel
    pub async fn publish_balance_update(
        &self,
        user_id: &str,
        balance_data: BalanceUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.publish_user_update(user_id, &UserUpdate::Balance(balance_data)).await
    }

    /// Publish order update to a specific user's private channel
    pub async fn publish_order_update(
        &self,
        user_id: &str,
        order_data: OrderUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.publish_user_update(user_id, &UserUpdate::Order(order_data)).await
    }

    /// Publish position update to a specific user's private channel
    pub async fn publish_position_update(
        &self,
        user_id: &str,
        position_data: PositionUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.publish_user_update(user_id, &UserUpdate::Position(position_data)).await
    }

    /// Generic publish method for any channel and data
    pub async fn publish<T: serde::Serialize>(
        &self,
        channel: &str,
        data: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = json!({
            "channel": channel,
            "data": data
        });

        self.send_request("publish", &body).await
    }

    /// Publish to multiple users at once (batch)
    pub async fn publish_to_users<T: serde::Serialize + Clone>(
        &self,
        user_ids: &[String],
        channel_suffix: &str,
        data: &T,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let commands: Vec<serde_json::Value> = user_ids
            .iter()
            .map(|user_id| {
                json!({
                    "publish": {
                        "channel": format!("user:{}#{}", user_id, channel_suffix),
                        "data": data.clone()
                    }
                })
            })
            .collect();

        let body = json!({
            "commands": commands
        });

        self.send_request("batch", &body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BalanceUpdate, OrderUpdate, PositionUpdate}; // Import for test serialization

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
        let balance =
            BalanceUpdate { asset: "BTC".to_string(), available: 1.5, locked: 0.2, total: 1.7 };

        let json = serde_json::to_string(&balance).unwrap();
        assert!(json.contains("BTC"));
        assert!(json.contains("1.5"));
    }

    #[test]
    fn test_order_update_serialization() {
        let order = OrderUpdate {
            order_id: "order_123".to_string(),
            symbol_id: 1,
            side: "buy".to_string(),
            order_type: "limit".to_string(),
            status: "new".to_string(),
            price: 3000.0,
            quantity: 1.5,
            filled_quantity: 0.0,
            remaining_quantity: 1.5,
        };

        let json = serde_json::to_string(&order).unwrap();
        assert!(json.contains("order_123"));
        assert!(json.contains("1"));
        assert!(json.contains("3000"));
    }

    #[test]
    fn test_position_update_serialization() {
        let position = PositionUpdate {
            symbol_id: 2,
            side: "long".to_string(),
            quantity: 0.5,
            entry_price: 50000.0,
            mark_price: 51000.0,
            liquidation_price: 40000.0,
            unrealized_pnl: 500.0,
            leverage: 10.0,
        };

        let json = serde_json::to_string(&position).unwrap();
        assert!(json.contains("2"));
        assert!(json.contains("long"));
        assert!(json.contains("50000"));
    }
}
