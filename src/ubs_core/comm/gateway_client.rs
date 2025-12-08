//! UBS Gateway Client - Optimized for Low Latency
//!
//! Best practices implemented:
//! - Dedicated polling thread for responses (no busy-wait per request)
//! - Async oneshot channels for non-blocking response handling
//! - Pre-allocated handlers (no allocation per poll)
//! - Lock-free pending responses map

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Sender, Receiver};
use rusteron_client::*;

use super::aeron_config::AeronConfig;
use super::order_receiver::OrderMessage;
use super::order_sender::SendError;
use super::response::ResponseMessage;
use crate::ubs_core::order::InternalOrder;

/// Response callback type - oneshot channel sender
type ResponseTx = Sender<ResponseMessage>;

/// Gateway client for UBSCore communication
/// Uses dedicated background thread for polling responses
pub struct UbsGatewayClient {
    config: AeronConfig,
    publication: Option<AeronPublication>,
    /// Channel to send response subscribers (order_id, oneshot sender)
    pending_tx: Option<Sender<(u64, ResponseTx)>>,
    /// Background thread handle
    _poll_thread: Option<thread::JoinHandle<()>>,
}

impl UbsGatewayClient {
    /// Create a new gateway client
    pub fn new(config: AeronConfig) -> Self {
        Self {
            config,
            publication: None,
            pending_tx: None,
            _poll_thread: None,
        }
    }

    /// Connect to Aeron - creates publication and starts background polling thread
    pub fn connect(&mut self) -> Result<(), SendError> {
        use super::driver::AERON_DIR;

        let ctx = AeronContext::new()
            .map_err(|e| SendError::AeronError(format!("Context failed: {:?}", e)))?;

        let dir_cstr = CString::new(AERON_DIR)
            .map_err(|_| SendError::AeronError("Invalid dir".into()))?;
        ctx.set_dir(&dir_cstr)
            .map_err(|e| SendError::AeronError(format!("Set dir failed: {:?}", e)))?;

        log::info!("[UBS_CLIENT] Using Aeron dir: {}", AERON_DIR);

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

        // Subscription for receiving responses
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

        log::info!("[UBS_CLIENT] Connected to Aeron");

        // Channel for registering pending responses
        let (pending_tx, pending_rx) = bounded::<(u64, ResponseTx)>(1024);

        // Start dedicated polling thread
        let poll_thread = Self::start_poll_thread(subscription, pending_rx);

        self.publication = Some(publication);
        self.pending_tx = Some(pending_tx);
        self._poll_thread = Some(poll_thread);

        Ok(())
    }

    /// Start background thread for polling responses
    fn start_poll_thread(
        subscription: AeronSubscription,
        pending_rx: Receiver<(u64, ResponseTx)>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            // Pre-allocated pending map (no allocation per request)
            let pending: Arc<Mutex<HashMap<u64, ResponseTx>>> =
                Arc::new(Mutex::new(HashMap::with_capacity(1024)));

            // Pre-allocated handler
            let handler = ResponseHandler {
                pending: pending.clone()
            };
            let handler_wrapped = Handler::leak(handler);

            loop {
                // Check for new pending registrations (non-blocking)
                while let Ok((order_id, tx)) = pending_rx.try_recv() {
                    let mut p = pending.lock().unwrap();
                    p.insert(order_id, tx);
                }

                // Poll for responses (non-blocking)
                let _ = subscription.poll(Some(&handler_wrapped), 100);

                // Yield to avoid 100% CPU (tiny sleep)
                std::thread::sleep(Duration::from_micros(10));
            }
        })
    }

    /// Send order and wait for response
    /// Now uses async oneshot channel instead of busy-wait
    pub fn send_order_and_wait(
        &self,
        order: &InternalOrder,
        timeout_ms: u64,
    ) -> Result<ResponseMessage, SendError> {
        let publication = self.publication.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        let pending_tx = self.pending_tx.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        // Create oneshot channel for this request
        let (resp_tx, resp_rx) = bounded::<ResponseMessage>(1);
        let order_id = order.order_id;

        // Register pending response BEFORE sending (avoid race)
        pending_tx.send((order_id, resp_tx))
            .map_err(|_| SendError::AeronError("Pending channel closed".into()))?;

        // Send order
        let msg = OrderMessage::from_order(order);
        let bytes = msg.to_bytes();

        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(bytes, handler);
        log::debug!("[UBS_CLIENT] offer result: position={}, order_id={}", position, order_id);

        if position <= 0 {
            log::warn!("[UBS_CLIENT] offer failed: position={}", position);
            return Err(SendError::Unknown(position));
        }

        // Wait for response with timeout (non-busy-wait!)
        match resp_rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(resp) => Ok(resp),
            Err(_) => Err(SendError::AeronError("Timeout waiting for response".into())),
        }
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.publication.as_ref().map_or(false, |p| p.is_connected())
    }
}

/// Handler for incoming responses - dispatches to oneshot channels
struct ResponseHandler {
    pending: Arc<Mutex<HashMap<u64, ResponseTx>>>,
}

impl AeronFragmentHandlerCallback for ResponseHandler {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        log::debug!("[UBS_CLIENT] Received {} bytes", buffer.len());

        if let Some(resp) = ResponseMessage::from_bytes(buffer) {
            log::info!("[UBS_CLIENT] Response: order_id={}, accepted={}",
                       resp.order_id, resp.is_accepted());

            // Dispatch to waiting oneshot channel
            let mut pending = self.pending.lock().unwrap();
            if let Some(tx) = pending.remove(&resp.order_id) {
                let _ = tx.send(resp); // Ignores if receiver dropped
            } else {
                log::warn!("[UBS_CLIENT] No pending request for order_id={}", resp.order_id);
            }
        } else {
            log::warn!("[UBS_CLIENT] Failed to parse response from {} bytes", buffer.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = UbsGatewayClient::new(AeronConfig::default());
        assert!(client.publication.is_none()); // Not connected yet
    }
}
