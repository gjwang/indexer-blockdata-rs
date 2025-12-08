//! Fill receiver from Matching Engine via Aeron IPC
//!
//! Subscribes to trade fills and applies balance changes.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusteron_client::*;

use super::aeron_config::AeronConfig;

/// Trade fill message from Matching Engine
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FillMessage {
    pub trade_id: u64,
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,     // 0 = Buy, 1 = Sell
    pub is_maker: u8, // 1 = maker, 0 = taker
    pub price: u64,
    pub qty: u64,
    pub quote_qty: u64, // price * qty (for convenience)
    pub fee: u64,
    pub timestamp_ns: u64,
}

impl FillMessage {
    /// Convert to bytes for transmission
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

    /// Start receiving fills (blocking)
    /// Calls the handler for each fill received
    pub fn start<F>(&self, mut handler: F) -> Result<(), String>
    where
        F: FnMut(FillMessage),
    {
        self.running.store(true, Ordering::SeqCst);

        let ctx = AeronContext::new().map_err(|e| format!("Aeron context error: {:?}", e))?;

        let aeron = Aeron::new(&ctx).map_err(|e| format!("Aeron client error: {:?}", e))?;

        aeron.start().map_err(|e| format!("Aeron start error: {:?}", e))?;

        let channel = CString::new(self.config.channel.as_str()).unwrap();
        let subscription = aeron
            .add_subscription(
                &channel,
                self.config.fills_in_stream,
                Handlers::no_available_image_handler(),
                Handlers::no_unavailable_image_handler(),
            )
            .map_err(|e| format!("Subscription error: {:?}", e))?;

        log::info!(
            "FillReceiver: subscribed to {} stream {}",
            self.config.channel,
            self.config.fills_in_stream
        );

        let fragment_handler = move |buffer: &[u8], _header: AeronHeader| {
            if let Some(fill) = FillMessage::from_bytes(buffer) {
                handler(fill);
            }
        };

        let (mut closure, _holder) =
            Handler::leak_with_fragment_assembler(fragment_handler).unwrap();

        while self.running.load(Ordering::Relaxed) {
            let _ = subscription.poll(Some(&mut closure), 10);

            if !self.config.busy_spin {
                std::thread::yield_now();
            }
        }

        log::info!("FillReceiver: stopped");
        Ok(())
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
        assert_eq!(parsed.user_id, msg.user_id);
        assert_eq!(parsed.fee, msg.fee);
    }

    #[test]
    fn test_fill_message_size() {
        // Verify struct size for wire format
        assert_eq!(std::mem::size_of::<FillMessage>(), 72);
    }
}
