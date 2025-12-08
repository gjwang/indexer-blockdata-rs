//! UBS Gateway Client
//!
//! High-level client for UBSCore communication.
//! Built on top of the generic AeronChannel.
//!
//! The AeronChannel handles all correlation internally -
//! this client just sends order bytes and gets response bytes.

use super::aeron_channel::{AeronChannel, AeronChannelConfig};
use super::aeron_config::AeronConfig;
use super::order_receiver::OrderMessage;
use super::order_sender::SendError;
use super::response::ResponseMessage;
use crate::ubs_core::order::InternalOrder;

/// Gateway client for UBSCore communication
/// Uses AeronChannel internally for transport
pub struct UbsGatewayClient {
    channel: AeronChannel,
}

impl UbsGatewayClient {
    /// Create a new gateway client
    pub fn new(config: AeronConfig) -> Self {
        let channel_config = AeronChannelConfig {
            send_channel: config.orders_channel.clone(),
            send_stream: config.orders_in_stream,
            recv_channel: config.responses_channel.clone(),
            recv_stream: config.responses_out_stream,
            poll_interval_us: 10,
        };

        Self {
            channel: AeronChannel::new(channel_config),
        }
    }

    /// Connect to Aeron
    pub fn connect(&mut self) -> Result<(), SendError> {
        self.channel.connect()?;
        log::info!("[UBS_CLIENT] Connected to Aeron");
        Ok(())
    }

    /// Send order and wait for response (blocking)
    pub fn send_order_and_wait(
        &self,
        order: &InternalOrder,
        timeout_ms: u64,
    ) -> Result<ResponseMessage, SendError> {
        // Serialize order
        let msg = OrderMessage::from_order(order);
        let payload = msg.to_bytes();

        log::debug!("[UBS_CLIENT] Sending order_id={}", order.order_id);

        // Send via channel
        let response_bytes = self.channel.send_and_receive(&payload, timeout_ms)?;

        // Parse response
        ResponseMessage::from_bytes(&response_bytes)
            .ok_or_else(|| SendError::AeronError("Invalid response format".into()))
    }

    /// Send order and wait for response (async)
    pub async fn send_order_async(
        &self,
        order: &InternalOrder,
        timeout_ms: u64,
    ) -> Result<ResponseMessage, SendError> {
        // Serialize order
        let msg = OrderMessage::from_order(order);
        let payload = msg.to_bytes();

        log::debug!("[UBS_CLIENT] Sending order_id={} (async)", order.order_id);

        // Send via channel (async)
        let response_bytes = self.channel.send_and_receive_async(&payload, timeout_ms).await?;

        // Parse response
        ResponseMessage::from_bytes(&response_bytes)
            .ok_or_else(|| SendError::AeronError("Invalid response format".into()))
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.channel.is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = UbsGatewayClient::new(AeronConfig::default());
        assert!(!client.is_connected());
    }
}
