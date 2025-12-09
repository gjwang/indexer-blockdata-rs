//! Aeron Server - handles request/response with correlation
//!
//! Mirrors AeronChannel but for the server side.
//! Automatically extracts correlation ID and includes it in response.

use std::ffi::CString;
use std::time::Duration;

use rusteron_client::*;

use super::driver::AERON_DIR;
use super::order_sender::SendError;
use super::WireMessage;

/// Configuration for Aeron server
#[derive(Debug, Clone)]
pub struct AeronServerConfig {
    /// Channel URI for receiving requests
    pub recv_channel: String,
    /// Stream ID for receiving
    pub recv_stream: i32,
    /// Channel URI for sending responses
    pub send_channel: String,
    /// Stream ID for sending
    pub send_stream: i32,
}

impl Default for AeronServerConfig {
    fn default() -> Self {
        Self {
            recv_channel: "aeron:ipc".to_string(),
            recv_stream: 1001,
            send_channel: "aeron:ipc".to_string(),
            send_stream: 1002,
        }
    }
}

/// Incoming request with correlation ID already extracted
pub struct Request<T> {
    /// The correlation ID (for response routing)
    pub correlation_id: u64,
    /// The parsed request payload
    pub payload: T,
}

/// Aeron server that handles request/response correlation
pub struct AeronServer {
    config: AeronServerConfig,
    subscription: Option<AeronSubscription>,
    publication: Option<AeronPublication>,
}

impl AeronServer {
    /// Create new server
    pub fn new(config: AeronServerConfig) -> Self {
        Self {
            config,
            subscription: None,
            publication: None,
        }
    }

    /// Create with default IPC config
    pub fn ipc(recv_stream: i32, send_stream: i32) -> Self {
        Self::new(AeronServerConfig {
            recv_channel: "aeron:ipc".to_string(),
            recv_stream,
            send_channel: "aeron:ipc".to_string(),
            send_stream,
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

        let aeron = Aeron::new(&ctx)
            .map_err(|e| SendError::AeronError(format!("Aeron failed: {:?}", e)))?;

        aeron.start()
            .map_err(|e| SendError::AeronError(format!("Start failed: {:?}", e)))?;

        // Subscription for receiving requests
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

        // Publication for sending responses
        let send_channel = CString::new(self.config.send_channel.clone())
            .map_err(|_| SendError::AeronError("Invalid channel".into()))?;

        let publication = aeron
            .add_publication(&send_channel, self.config.send_stream, Duration::from_secs(5))
            .map_err(|e| SendError::AeronError(format!("Publication failed: {:?}", e)))?;

        log::info!("[AERON_SERVER] Subscription on {} stream {}",
            self.config.recv_channel, self.config.recv_stream);
        log::info!("[AERON_SERVER] Publication on {} stream {}",
            self.config.send_channel, self.config.send_stream);

        self.subscription = Some(subscription);
        self.publication = Some(publication);

        Ok(())
    }

    /// Get subscription for polling
    pub fn subscription(&self) -> Option<&AeronSubscription> {
        self.subscription.as_ref()
    }

    /// Send response with correlation ID
    ///
    /// Protocol: [8-byte correlation_id LE] + [response_payload]
    pub fn send_response<R: WireMessage>(&self, correlation_id: u64, response: &R) -> Result<(), SendError> {
        let publication = self.publication.as_ref()
            .ok_or_else(|| SendError::AeronError("Not connected".into()))?;

        let resp_bytes = response.to_bytes();

        // Build message: [correlation_id] + [response_payload]
        let mut message = Vec::with_capacity(8 + resp_bytes.len());
        message.extend_from_slice(&correlation_id.to_le_bytes());
        message.extend_from_slice(resp_bytes);

        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(&message, handler);

        if position <= 0 {
            return Err(SendError::Unknown(position));
        }

        Ok(())
    }
}

/// Parse incoming buffer to extract correlation ID and payload
///
/// Protocol: [8-byte correlation_id LE] + [payload]
/// Returns (correlation_id, payload_bytes) or None if invalid
pub fn parse_request(buffer: &[u8]) -> Option<(u64, &[u8])> {
    if buffer.len() < 8 {
        return None;
    }

    let mut id_bytes = [0u8; 8];
    id_bytes.copy_from_slice(&buffer[0..8]);
    let correlation_id = u64::from_le_bytes(id_bytes);

    Some((correlation_id, &buffer[8..]))
}
