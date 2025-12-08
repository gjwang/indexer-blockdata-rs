//! Generic Aeron IPC Channel
//!
//! A payload-agnostic communication channel using Aeron IPC.
//! The channel doesn't know or care about the message format.
//! Caller provides: bytes in, gets bytes out.
//!
//! Features:
//! - Request/Response correlation by ID
//! - Dedicated background polling thread
//! - Async response handling via oneshot channels

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Sender, Receiver};
use rusteron_client::*;

use super::driver::AERON_DIR;
use super::order_sender::SendError;

/// Response callback type - sends raw bytes back
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

/// Trait for extracting correlation ID from response bytes
/// Implement this for your specific message format
pub trait CorrelationExtractor: Send + Sync + 'static {
    /// Extract correlation ID from response bytes
    /// Returns None if response is invalid
    fn extract_correlation_id(&self, bytes: &[u8]) -> Option<u64>;
}

/// Default extractor - assumes first 8 bytes are correlation ID (little-endian)
pub struct DefaultCorrelationExtractor;

impl CorrelationExtractor for DefaultCorrelationExtractor {
    fn extract_correlation_id(&self, bytes: &[u8]) -> Option<u64> {
        if bytes.len() < 8 {
            return None;
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&bytes[0..8]);
        Some(u64::from_le_bytes(arr))
    }
}

/// Generic Aeron IPC channel for request/response communication
pub struct AeronChannel {
    config: AeronChannelConfig,
    publication: Option<AeronPublication>,
    /// Channel for registering pending responses (correlation_id, oneshot sender)
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

    /// Connect to Aeron with a custom correlation extractor
    pub fn connect_with_extractor<E: CorrelationExtractor>(
        &mut self,
        extractor: E,
    ) -> Result<(), SendError> {
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
        let poll_thread = Self::start_poll_thread(subscription, pending_rx, extractor, poll_interval);

        self.publication = Some(publication);
        self.pending_tx = Some(pending_tx);
        self._poll_thread = Some(poll_thread);

        Ok(())
    }

    /// Connect with default correlation extractor (first 8 bytes = ID)
    pub fn connect(&mut self) -> Result<(), SendError> {
        self.connect_with_extractor(DefaultCorrelationExtractor)
    }

    /// Start background thread for polling responses
    fn start_poll_thread<E: CorrelationExtractor>(
        subscription: AeronSubscription,
        pending_rx: Receiver<(u64, ResponseTx)>,
        extractor: E,
        poll_interval_us: u64,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let pending: Arc<Mutex<HashMap<u64, ResponseTx>>> =
                Arc::new(Mutex::new(HashMap::with_capacity(1024)));
            let extractor = Arc::new(extractor);

            let handler = GenericResponseHandler {
                pending: pending.clone(),
                extractor: extractor.clone(),
            };
            let handler_wrapped = Handler::leak(handler);

            loop {
                // Check for new pending registrations
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

    /// Send bytes and wait for response
    /// correlation_id: ID that will be matched in the response
    /// payload: raw bytes to send
    pub fn send_and_receive(
        &self,
        correlation_id: u64,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, SendError> {
        let publication = self.publication.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        let pending_tx = self.pending_tx.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        // Create response channel
        let (resp_tx, resp_rx) = bounded::<Vec<u8>>(1);

        // Register pending BEFORE sending
        pending_tx.send((correlation_id, resp_tx))
            .map_err(|_| SendError::AeronError("Pending channel closed".into()))?;

        // Send payload
        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(payload, handler);

        if position <= 0 {
            log::warn!("[AERON_CHANNEL] offer failed: position={}", position);
            return Err(SendError::Unknown(position));
        }

        // Wait for response
        match resp_rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(bytes) => Ok(bytes),
            Err(_) => Err(SendError::AeronError("Timeout".into())),
        }
    }

    /// Send bytes without waiting for response (fire-and-forget)
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

/// Generic response handler - dispatches to waiting channels
struct GenericResponseHandler<E: CorrelationExtractor> {
    pending: Arc<Mutex<HashMap<u64, ResponseTx>>>,
    extractor: Arc<E>,
}

impl<E: CorrelationExtractor> AeronFragmentHandlerCallback for GenericResponseHandler<E> {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        // Extract correlation ID using the provided extractor
        if let Some(id) = self.extractor.extract_correlation_id(buffer) {
            let mut pending = self.pending.lock().unwrap();
            if let Some(tx) = pending.remove(&id) {
                let _ = tx.send(buffer.to_vec());
            } else {
                log::debug!("[AERON_CHANNEL] No pending request for id={}", id);
            }
        } else {
            log::warn!("[AERON_CHANNEL] Failed to extract correlation ID from {} bytes", buffer.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_extractor() {
        let extractor = DefaultCorrelationExtractor;

        // Valid: 8 bytes
        let bytes = 12345u64.to_le_bytes();
        assert_eq!(extractor.extract_correlation_id(&bytes), Some(12345));

        // Invalid: too short
        assert_eq!(extractor.extract_correlation_id(&[1, 2, 3]), None);
    }

    #[test]
    fn test_channel_creation() {
        let channel = AeronChannel::ipc(1001, 1002);
        assert!(!channel.is_connected());
    }
}
