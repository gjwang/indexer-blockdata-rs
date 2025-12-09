//! Generic Aeron IPC Channel
//!
//! A **fully payload-agnostic** communication channel using Aeron IPC.
//!
//! The channel:
//! - Generates its own correlation IDs internally
//! - Wraps outgoing payloads with correlation ID
//! - Strips correlation ID from responses
//! - Caller just sees: send(payload) -> response
//!
//! Protocol:
//! - Outgoing: [8-byte correlation_id LE] + [payload]
//! - Incoming: [8-byte correlation_id LE] + [response]

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Sender, Receiver};
use rusteron_client::*;

use super::driver::AERON_DIR;
use super::order_sender::SendError;

/// Response callback type - sends raw response bytes (without correlation header)
type ResponseTx = Sender<Vec<u8>>;

/// Configuration for Aeron channel
#[derive(Debug, Clone)]
pub struct AeronChannelConfig {
    /// Channel URI for sending (e.g., "aeron:ipc")
    pub send_channel: String,
    /// Stream ID for sending
    pub send_stream: i32,
    /// Channel URI for receiving
    pub recv_channel: String,
    /// Stream ID for receiving
    pub recv_stream: i32,
    /// Poll interval in microseconds
    pub poll_interval_us: u64,
}

impl Default for AeronChannelConfig {
    fn default() -> Self {
        Self {
            send_channel: "aeron:ipc".to_string(),
            send_stream: 1001,
            recv_channel: "aeron:ipc".to_string(),
            recv_stream: 1002,
            poll_interval_us: 10,
        }
    }
}

/// Generic Aeron IPC channel for request/response communication
///
/// Caller-agnostic: just sends bytes, gets bytes back.
/// Correlation is handled internally using auto-generated IDs.
pub struct AeronChannel {
    config: AeronChannelConfig,
    publication: Option<AeronPublication>,
    /// Sequence generator for correlation IDs
    next_correlation_id: Arc<AtomicU64>,
    /// Channel for registering pending responses
    pending_tx: Option<Sender<(u64, ResponseTx)>>,
    /// Background polling thread
    _poll_thread: Option<thread::JoinHandle<()>>,
}

impl AeronChannel {
    /// Create a new channel
    pub fn new(config: AeronChannelConfig) -> Self {
        Self {
            config,
            publication: None,
            next_correlation_id: Arc::new(AtomicU64::new(1)),
            pending_tx: None,
            _poll_thread: None,
        }
    }

    /// Create with default IPC config
    pub fn ipc(send_stream: i32, recv_stream: i32) -> Self {
        Self::new(AeronChannelConfig {
            send_channel: "aeron:ipc".to_string(),
            send_stream,
            recv_channel: "aeron:ipc".to_string(),
            recv_stream,
            poll_interval_us: 10,
        })
    }

    /// Connect to Aeron
    pub fn connect(&mut self) -> Result<(), SendError> {
        let ctx = AeronContext::new()
            .map_err(|e| SendError::AeronError(format!("Context failed: {:?}", e)))?;

        let dir_cstr = CString::new(AERON_DIR)
            .map_err(|_| SendError::AeronError("Invalid dir".into()))?;
        ctx.set_dir(&dir_cstr)
            .map_err(|e| SendError::AeronError(format!("Set dir failed: {:?}", e)))?;

        log::info!("[AERON_CHANNEL] Using dir: {}", AERON_DIR);

        let aeron = Aeron::new(&ctx)
            .map_err(|e| SendError::AeronError(format!("Aeron failed: {:?}", e)))?;

        aeron.start()
            .map_err(|e| SendError::AeronError(format!("Start failed: {:?}", e)))?;

        // Publication for sending
        let send_channel = CString::new(self.config.send_channel.clone())
            .map_err(|_| SendError::AeronError("Invalid channel".into()))?;

        let publication = aeron
            .add_publication(&send_channel, self.config.send_stream, Duration::from_secs(5))
            .map_err(|e| SendError::AeronError(format!("Publication failed: {:?}", e)))?;

        // Subscription for receiving
        let recv_channel = CString::new(self.config.recv_channel.clone())
            .map_err(|_| SendError::AeronError("Invalid channel".into()))?;

        let handler_avail: Option<&Handler<AeronAvailableImageLogger>> = None;
        let handler_unavail: Option<&Handler<AeronUnavailableImageLogger>> = None;

        let subscription = aeron
            .add_subscription(
                &recv_channel,
                self.config.recv_stream,
                handler_avail,
                handler_unavail,
                Duration::from_secs(5),
            )
            .map_err(|e| SendError::AeronError(format!("Subscription failed: {:?}", e)))?;

        log::info!("[AERON_CHANNEL] Connected");

        // Channel for registering pending responses
        let (pending_tx, pending_rx) = bounded::<(u64, ResponseTx)>(1024);

        // Start background polling thread
        let poll_interval = self.config.poll_interval_us;
        let poll_thread = Self::start_poll_thread(subscription, pending_rx, poll_interval);

        self.publication = Some(publication);
        self.pending_tx = Some(pending_tx);
        self._poll_thread = Some(poll_thread);

        Ok(())
    }

    /// Start background thread for polling responses
    fn start_poll_thread(
        subscription: AeronSubscription,
        pending_rx: Receiver<(u64, ResponseTx)>,
        poll_interval_us: u64,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let pending: Arc<Mutex<HashMap<u64, ResponseTx>>> =
                Arc::new(Mutex::new(HashMap::with_capacity(1024)));

            let handler = CorrelationHandler {
                pending: pending.clone(),
            };
            let handler_wrapped = Handler::leak(handler);

            loop {
                // Check for new pending registrations (non-blocking)
                while let Ok((id, tx)) = pending_rx.try_recv() {
                    let mut p = pending.lock().unwrap();
                    p.insert(id, tx);
                }

                // Poll for responses
                let _ = subscription.poll(Some(&handler_wrapped), 100);

                // Small yield
                std::thread::sleep(Duration::from_micros(poll_interval_us));
            }
        })
    }

    /// Generate next correlation ID
    fn next_id(&self) -> u64 {
        self.next_correlation_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send payload bytes and wait for response bytes (async)
    ///
    /// Caller doesn't need to know about correlation - it's handled internally.
    /// Protocol: sends [8-byte correlation_id] + [payload]
    ///           expects [8-byte correlation_id] + [response]
    pub async fn send_and_receive(
        &self,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, SendError> {
        let publication = self.publication.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        let pending_tx = self.pending_tx.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        // Generate correlation ID
        let correlation_id = self.next_id();

        // Create response channel (crossbeam for thread-safety with poll thread)
        let (resp_tx, resp_rx) = bounded::<Vec<u8>>(1);

        // Register pending BEFORE sending
        pending_tx.send((correlation_id, resp_tx))
            .map_err(|_| SendError::AeronError("Pending channel closed".into()))?;

        // Build message: [correlation_id (8 bytes LE)] + [payload]
        let mut message = Vec::with_capacity(8 + payload.len());
        message.extend_from_slice(&correlation_id.to_le_bytes());
        message.extend_from_slice(payload);

        // Send
        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(&message, handler);

        if position <= 0 {
            log::warn!("[AERON_CHANNEL] offer failed: position={}", position);
            return Err(SendError::Unknown(position));
        }

        // Wait for response using tokio timeout
        let timeout = Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, tokio::task::spawn_blocking(move || {
            resp_rx.recv()
        })).await {
            Ok(Ok(Ok(bytes))) => Ok(bytes),
            _ => Err(SendError::AeronError("Timeout".into())),
        }
    }

    /// Send payload without waiting for response (fire-and-forget)
    /// No correlation ID added - pure fire-and-forget
    pub fn send(&self, payload: &[u8]) -> Result<i64, SendError> {
        let publication = self.publication.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(payload, handler);

        if position <= 0 {
            return Err(SendError::Unknown(position));
        }

        Ok(position)
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.publication.as_ref().map_or(false, |p| p.is_connected())
    }
}

/// Handler that extracts correlation ID from first 8 bytes and dispatches response
struct CorrelationHandler {
    pending: Arc<Mutex<HashMap<u64, ResponseTx>>>,
}

impl AeronFragmentHandlerCallback for CorrelationHandler {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        // Message format: [8-byte correlation_id LE] + [response_payload]
        if buffer.len() < 8 {
            log::warn!("[AERON_CHANNEL] Response too short: {} bytes", buffer.len());
            return;
        }

        // Extract correlation ID
        let mut id_bytes = [0u8; 8];
        id_bytes.copy_from_slice(&buffer[0..8]);
        let correlation_id = u64::from_le_bytes(id_bytes);

        // Extract response payload (skip correlation header)
        let response_payload = buffer[8..].to_vec();

        // Dispatch to waiting channel
        let mut pending = self.pending.lock().unwrap();
        if let Some(tx) = pending.remove(&correlation_id) {
            let _ = tx.send(response_payload);
        } else {
            log::debug!("[AERON_CHANNEL] No pending request for correlation_id={}", correlation_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_creation() {
        let channel = AeronChannel::ipc(1001, 1002);
        assert!(!channel.is_connected());
    }

    #[test]
    fn test_correlation_id_generation() {
        let channel = AeronChannel::ipc(1001, 1002);
        let id1 = channel.next_id();
        let id2 = channel.next_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }
}
