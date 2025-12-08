//! Fill receiver from Matching Engine via Aeron IPC
//!
//! Subscribes to trade fills and applies balance changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusteron_client::*;

use super::aeron_config::AeronConfig;
use super::order_receiver::{AeronNoAvailableImageHandler, AeronNoUnavailableImageHandler};

/// Trade fill message from Matching Engine
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FillMessage {
    pub trade_id: u64,
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,
    pub is_maker: u8,
    pub price: u64,
    pub qty: u64,
    pub quote_qty: u64,
    pub fee: u64,
    pub timestamp_ns: u64,
}

impl FillMessage {
    /// Convert to bytes
    pub fn to_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }

    /// Parse from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != std::mem::size_of::<Self>() {
            return None;
        }
        unsafe {
            let ptr = bytes.as_ptr() as *const Self;
            Some(ptr.read())
        }
    }
}

/// Fill receiver using Aeron subscription
pub struct FillReceiver {
    config: AeronConfig,
    running: Arc<AtomicBool>,
}

impl FillReceiver {
    /// Create a new fill receiver
    pub fn new(config: AeronConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)) }
    }

    /// Get running flag for shutdown
    pub fn running(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Get config
    pub fn config(&self) -> &AeronConfig {
        &self.config
    }

    /// Stop the receiver
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fill_message_roundtrip() {
        let msg = FillMessage {
            trade_id: 1,
            order_id: 12345,
            user_id: 1001,
            symbol_id: 1,
            side: 0,
            is_maker: 1,
            price: 50000_00000000,
            qty: 1_00000000,
            quote_qty: 50000_00000000,
            fee: 50_00000000,
            timestamp_ns: 1234567890,
        };

        let bytes = msg.to_bytes();
        let parsed = FillMessage::from_bytes(bytes).unwrap();

        assert_eq!(parsed.trade_id, msg.trade_id);
        assert_eq!(parsed.fee, msg.fee);
    }

    #[test]
    fn test_fill_message_size() {
        assert_eq!(std::mem::size_of::<FillMessage>(), 72);
    }
}
