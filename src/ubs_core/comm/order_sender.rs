//! Order sender to UBSCore via Aeron UDP
//!
//! Gateway uses this to publish orders to UBSCore.

use std::ffi::CString;
use std::sync::Arc;
use std::time::Duration;

use rusteron_client::*;

use super::aeron_config::AeronConfig;
use super::order_receiver::OrderMessage;
use super::WireMessage;  // For to_bytes
use crate::ubs_core::order::InternalOrder;

/// Order sender using Aeron publication
pub struct OrderSender {
    config: AeronConfig,
    aeron: Option<Arc<Aeron>>,
    publication: Option<AeronPublication>,
}

impl OrderSender {
    /// Create a new order sender
    pub fn new(config: AeronConfig) -> Self {
        Self {
            config,
            aeron: None,
            publication: None,
        }
    }

    /// Connect to Aeron and create publication
    pub fn connect(&mut self) -> Result<(), SendError> {
        // Create Aeron context
        let ctx = AeronContext::new()
            .map_err(|e| SendError::AeronError(format!("Context creation failed: {:?}", e)))?;

        // Connect to Aeron driver
        let aeron = Aeron::new(&ctx)
            .map_err(|e| SendError::AeronError(format!("Aeron connection failed: {:?}", e)))?;

        aeron.start()
            .map_err(|e| SendError::AeronError(format!("Aeron start failed: {:?}", e)))?;

        // Create publication (blocks until connected or timeout)
        let channel = CString::new(self.config.orders_channel.clone())
            .map_err(|_| SendError::AeronError("Invalid channel string".into()))?;

        let publication = aeron
            .add_publication(&channel, self.config.orders_in_stream, Duration::from_secs(5))
            .map_err(|e| SendError::AeronError(format!("Add publication failed: {:?}", e)))?;

        self.aeron = Some(Arc::new(aeron));
        self.publication = Some(publication);

        log::info!("[AERON] Connected to {}", self.config.orders_channel);
        Ok(())
    }

    /// Send an order to UBSCore
    ///
    /// Aeron offer return codes:
    /// - >= 0: Success (position)
    /// - -1: NOT_CONNECTED
    /// - -2: BACK_PRESSURED (buffer full)
    /// - -3: ADMIN_ACTION
    /// - -4: CLOSED
    /// - -5: MAX_POSITION_EXCEEDED
    pub fn send(&self, order: &InternalOrder) -> Result<i64, SendError> {
        let publication = self.publication.as_ref().ok_or(SendError::NotConnected)?;

        let msg = OrderMessage::from_order(order);
        let bytes = msg.to_bytes();

        // offer returns position (i64): positive = success, negative = error code
        let handler: Option<&Handler<AeronReservedValueSupplierLogger>> = None;
        let position = publication.offer(bytes, handler);

        match position {
            p if p >= 0 => Ok(p),
            -1 => Err(SendError::NotConnected),
            -2 => Err(SendError::BackPressured),  // Buffer full!
            -3 => Err(SendError::AdminAction),
            -4 => Err(SendError::Closed),
            -5 => Err(SendError::MaxPositionExceeded),
            _ => Err(SendError::Unknown(position)),
        }
    }

    /// Get config
    pub fn config(&self) -> &AeronConfig {
        &self.config
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.publication.as_ref().map_or(false, |p| p.is_connected())
    }
}

/// Send errors
#[derive(Debug, Clone)]
pub enum SendError {
    NotConnected,
    BackPressured,   // Buffer full - respond with SystemBusy
    AdminAction,
    Closed,
    MaxPositionExceeded,
    MaxRetriesExceeded,
    AeronError(String),
    Unknown(i64),
}

impl SendError {
    /// Check if this is a back pressure (buffer full) error
    pub fn is_back_pressured(&self) -> bool {
        matches!(self, SendError::BackPressured)
    }

    /// Check if this is a "system busy" type error (should return 503)
    pub fn is_system_busy(&self) -> bool {
        matches!(self, SendError::BackPressured | SendError::AdminAction | SendError::MaxPositionExceeded)
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotConnected => write!(f, "Not connected to subscriber"),
            SendError::BackPressured => write!(f, "System busy - buffer full"),
            SendError::AdminAction => write!(f, "Admin action in progress"),
            SendError::Closed => write!(f, "Publication closed"),
            SendError::MaxPositionExceeded => write!(f, "Max position exceeded"),
            SendError::MaxRetriesExceeded => write!(f, "Max retries exceeded"),
            SendError::AeronError(e) => write!(f, "Aeron error: {}", e),
            SendError::Unknown(code) => write!(f, "Unknown error code: {}", code),
        }
    }
}

impl std::error::Error for SendError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_error_display() {
        assert_eq!(format!("{}", SendError::NotConnected), "Not connected to subscriber");
    }
}
