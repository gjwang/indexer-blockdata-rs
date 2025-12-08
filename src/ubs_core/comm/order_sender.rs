//! Order sender to Matching Engine via Aeron IPC
//!
//! Publishes validated orders to Matching Engine.

use std::ffi::CString;
use std::time::Duration;

use rusteron_client::*;

use super::aeron_config::AeronConfig;
use super::order_receiver::OrderMessage;
use crate::ubs_core::order::InternalOrder;

/// Order sender using Aeron publication
pub struct OrderSender {
    config: AeronConfig,
}

impl OrderSender {
    /// Create a new order sender (stores config for later use)
    pub fn new(config: AeronConfig) -> Self {
        Self { config }
    }

    /// Get config
    pub fn config(&self) -> &AeronConfig {
        &self.config
    }
}

/// Send errors
#[derive(Debug, Clone)]
pub enum SendError {
    NotConnected,
    BackPressured,
    AdminAction,
    MaxRetriesExceeded,
    AeronError(String),
    Unknown(i64),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotConnected => write!(f, "Not connected to subscriber"),
            SendError::BackPressured => write!(f, "Back pressure - subscriber slow"),
            SendError::AdminAction => write!(f, "Admin action in progress"),
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
