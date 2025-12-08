//! Order receiver from Gateway via Aeron IPC
//!
//! Subscribes to order requests and delivers them to UBSCore.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusteron_client::*;

use super::aeron_config::AeronConfig;
use crate::ubs_core::order::{InternalOrder, OrderType, Side};
use crate::ubs_core::RejectReason;

/// Message format for order requests (wire format)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OrderMessage {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,       // 0 = Buy, 1 = Sell
    pub order_type: u8, // 0 = Limit, 1 = Market
    pub price: u64,
    pub qty: u64,
}

impl OrderMessage {
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

    /// Convert to InternalOrder
    pub fn to_internal_order(&self) -> Result<InternalOrder, RejectReason> {
        let side = match self.side {
            0 => Side::Buy,
            1 => Side::Sell,
            _ => return Err(RejectReason::InvalidSymbol), // Reuse for invalid side
        };

        let order_type = match self.order_type {
            0 => OrderType::Limit,
            1 => OrderType::Market,
            _ => return Err(RejectReason::InvalidSymbol),
        };

        Ok(InternalOrder {
            order_id: self.order_id,
            user_id: self.user_id,
            symbol_id: self.symbol_id,
            side,
            price: self.price,
            qty: self.qty,
            order_type,
        })
    }
}

/// Order receiver using Aeron subscription
pub struct OrderReceiver {
    config: AeronConfig,
    running: Arc<AtomicBool>,
}

impl OrderReceiver {
    /// Create a new order receiver
    pub fn new(config: AeronConfig) -> Self {
        Self { config, running: Arc::new(AtomicBool::new(false)) }
    }

    /// Get running flag for shutdown
    pub fn running(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Start receiving orders (blocking)
    /// Calls the handler for each order received
    pub fn start<F>(&self, mut handler: F) -> Result<(), String>
    where
        F: FnMut(InternalOrder) -> Result<(), RejectReason>,
    {
        self.running.store(true, Ordering::SeqCst);

        // Create Aeron context and client
        let ctx = AeronContext::new().map_err(|e| format!("Aeron context error: {:?}", e))?;

        let aeron = Aeron::new(&ctx).map_err(|e| format!("Aeron client error: {:?}", e))?;

        aeron.start().map_err(|e| format!("Aeron start error: {:?}", e))?;

        // Subscribe to orders channel
        let channel = CString::new(self.config.channel.as_str()).unwrap();
        let subscription = aeron
            .add_subscription(
                &channel,
                self.config.orders_in_stream,
                Handlers::no_available_image_handler(),
                Handlers::no_unavailable_image_handler(),
            )
            .map_err(|e| format!("Subscription error: {:?}", e))?;

        log::info!(
            "OrderReceiver: subscribed to {} stream {}",
            self.config.channel,
            self.config.orders_in_stream
        );

        // Fragment handler
        let fragment_handler = move |buffer: &[u8], _header: AeronHeader| {
            if let Some(msg) = OrderMessage::from_bytes(buffer) {
                match msg.to_internal_order() {
                    Ok(order) => {
                        if let Err(reason) = handler(order) {
                            log::warn!("Order rejected: {:?}", reason);
                        }
                    }
                    Err(e) => {
                        log::warn!("Invalid order message: {:?}", e);
                    }
                }
            }
        };

        let (mut closure, _holder) =
            Handler::leak_with_fragment_assembler(fragment_handler).unwrap();

        // Poll loop
        while self.running.load(Ordering::Relaxed) {
            let _ = subscription.poll(Some(&mut closure), 10);

            if !self.config.busy_spin {
                std::thread::yield_now();
            }
        }

        log::info!("OrderReceiver: stopped");
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
    fn test_order_message_roundtrip() {
        let msg = OrderMessage {
            order_id: 12345,
            user_id: 1001,
            symbol_id: 1,
            side: 0,
            order_type: 0,
            price: 50000_00000000,
            qty: 1_00000000,
        };

        let bytes = msg.to_bytes();
        let parsed = OrderMessage::from_bytes(bytes).unwrap();

        assert_eq!(parsed.order_id, msg.order_id);
        assert_eq!(parsed.user_id, msg.user_id);
        assert_eq!(parsed.price, msg.price);
    }

    #[test]
    fn test_to_internal_order() {
        let msg = OrderMessage {
            order_id: 12345,
            user_id: 1001,
            symbol_id: 1,
            side: 0, // Buy
            order_type: 0,
            price: 50000,
            qty: 100,
        };

        let order = msg.to_internal_order().unwrap();
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.price, 50000);
    }
}
