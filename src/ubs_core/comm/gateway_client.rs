//! UBS Gateway Client
//!
//! Sends orders to UBSCore via Aeron UDP and receives responses.
//! Uses correlation by order_id.

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusteron_client::*;

use super::aeron_config::AeronConfig;
use super::order_receiver::OrderMessage;
use super::order_sender::SendError;
use super::response::ResponseMessage;
use crate::ubs_core::order::InternalOrder;

/// Gateway client for UBSCore communication
/// Handles request/response pattern via Aeron
pub struct UbsGatewayClient {
    config: AeronConfig,
    aeron: Option<Arc<Aeron>>,
    publication: Option<AeronPublication>,
    subscription: Option<AeronSubscription>,
    /// Pending responses indexed by order_id
    pending: Arc<Mutex<HashMap<u64, ResponseMessage>>>,
}

impl UbsGatewayClient {
    /// Create a new gateway client
    pub fn new(config: AeronConfig) -> Self {
        Self {
            config,
            aeron: None,
            publication: None,
            subscription: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Connect to Aeron - creates publication (orders) and subscription (responses)
    pub fn connect(&mut self) -> Result<(), SendError> {
        let ctx = AeronContext::new()
            .map_err(|e| SendError::AeronError(format!("Context failed: {:?}", e)))?;

        let aeron = Aeron::new(&ctx)
            .map_err(|e| SendError::AeronError(format!("Aeron failed: {:?}", e)))?;

        aeron.start()
            .map_err(|e| SendError::AeronError(format!("Start failed: {:?}", e)))?;

        // Publication for sending orders
        let orders_channel = CString::new(self.config.orders_channel.clone())
            .map_err(|_| SendError::AeronError("Invalid channel".into()))?;

        let publication = aeron
            .add_publication(&orders_channel, self.config.orders_in_stream, Duration::from_secs(5))
            .map_err(|e| SendError::AeronError(format!("Publication failed: {:?}", e)))?;

        // Subscription for receiving responses - pass None for optional handlers
        let responses_channel = CString::new(self.config.responses_channel.clone())
            .map_err(|_| SendError::AeronError("Invalid channel".into()))?;

        let handler_avail: Option<&Handler<AeronAvailableImageLogger>> = None;
        let handler_unavail: Option<&Handler<AeronUnavailableImageLogger>> = None;

        let subscription = aeron
            .add_subscription(
                &responses_channel,
                self.config.responses_out_stream,
                handler_avail,
                handler_unavail,
                Duration::from_secs(5),
            )
            .map_err(|e| SendError::AeronError(format!("Subscription failed: {:?}", e)))?;

        self.aeron = Some(Arc::new(aeron));
        self.publication = Some(publication);
        self.subscription = Some(subscription);

        log::info!("[UBS_CLIENT] Connected to Aeron");
        Ok(())
    }

    /// Send order and wait for response (blocking with timeout)
    pub fn send_order(&self, order: &InternalOrder, timeout_ms: u64) -> Result<ResponseMessage, SendError> {
        let publication = self.publication.as_ref().ok_or(SendError::NotConnected)?;
        let subscription = self.subscription.as_ref().ok_or(SendError::NotConnected)?;

        let msg = OrderMessage::from_order(order);
        let bytes = msg.to_bytes();

        // Send order
        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(bytes, handler);
        log::debug!("[UBS_CLIENT] offer result: position={}, order_id={}", position, order.order_id);

        if position <= 0 {
            log::warn!("[UBS_CLIENT] offer failed: position={}", position);
            return Err(SendError::Unknown(position));
        }

        // Wait for response (poll subscription)
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let order_id = order.order_id;

        while std::time::Instant::now() < deadline {
            // Check pending map first
            {
                let mut pending = self.pending.lock().unwrap();
                if let Some(resp) = pending.remove(&order_id) {
                    return Ok(resp);
                }
            }

            // Poll subscription - use closure to handle fragments
            let pending_clone = self.pending.clone();
            let handler = Handler::leak(ResponseHandler { pending: pending_clone });
            let _ = subscription.poll(Some(&handler), 10);

            std::thread::sleep(Duration::from_micros(100));
        }

        Err(SendError::AeronError("Timeout waiting for response".into()))
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.publication.as_ref().map_or(false, |p| p.is_connected())
    }
}

/// Handler for incoming responses
struct ResponseHandler {
    pending: Arc<Mutex<HashMap<u64, ResponseMessage>>>,
}

impl AeronFragmentHandlerCallback for ResponseHandler {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        if let Some(resp) = ResponseMessage::from_bytes(buffer) {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(resp.order_id, resp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = UbsGatewayClient::new(AeronConfig::default());
        assert!(client.aeron.is_none()); // Not connected yet
    }
}
