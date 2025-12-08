//! Order sender to Matching Engine via Aeron IPC
//!
//! Publishes validated orders to Matching Engine.

use std::ffi::CString;

use rusteron_client::*;

use super::aeron_config::AeronConfig;
use super::order_receiver::OrderMessage;
use crate::ubs_core::order::InternalOrder;

/// Order sender using Aeron publication
pub struct OrderSender {
    aeron: Aeron,
    publication: AeronPublication,
}

impl OrderSender {
    /// Create a new order sender
    pub fn new(config: &AeronConfig) -> Result<Self, String> {
        let ctx = AeronContext::new().map_err(|e| format!("Aeron context error: {:?}", e))?;

        let aeron = Aeron::new(&ctx).map_err(|e| format!("Aeron client error: {:?}", e))?;

        aeron.start().map_err(|e| format!("Aeron start error: {:?}", e))?;

        let channel = CString::new(config.channel.as_str()).unwrap();
        let publication = aeron
            .add_publication(&channel, config.orders_out_stream)
            .map_err(|e| format!("Publication error: {:?}", e))?;

        log::info!(
            "OrderSender: publishing to {} stream {}",
            config.channel,
            config.orders_out_stream
        );

        Ok(Self { aeron, publication })
    }

    /// Send validated order to Matching Engine
    /// Returns bytes sent or error
    pub fn send(&self, order: &InternalOrder) -> Result<i64, SendError> {
        let msg = OrderMessage {
            order_id: order.order_id,
            user_id: order.user_id,
            symbol_id: order.symbol_id,
            side: match order.side {
                crate::ubs_core::order::Side::Buy => 0,
                crate::ubs_core::order::Side::Sell => 1,
            },
            order_type: match order.order_type {
                crate::ubs_core::order::OrderType::Limit => 0,
                crate::ubs_core::order::OrderType::Market => 1,
            },
            price: order.price,
            qty: order.qty,
        };

        let buffer = msg.to_bytes();

        // offer() is non-blocking
        let result = self
            .publication
            .offer(buffer, Handlers::no_reserved_value_supplier_handler())
            .map_err(|e| SendError::AeronError(format!("{:?}", e)))?;

        match result {
            r if r > 0 => Ok(r),
            -1 => Err(SendError::NotConnected),
            -2 => Err(SendError::BackPressured),
            -3 => Err(SendError::AdminAction),
            _ => Err(SendError::Unknown(result)),
        }
    }

    /// Send with retry (blocks until success or max retries)
    pub fn send_with_retry(
        &self,
        order: &InternalOrder,
        max_retries: u32,
    ) -> Result<i64, SendError> {
        for attempt in 0..max_retries {
            match self.send(order) {
                Ok(bytes) => return Ok(bytes),
                Err(SendError::NotConnected) | Err(SendError::BackPressured) => {
                    if attempt < max_retries - 1 {
                        std::thread::sleep(std::time::Duration::from_micros(100));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(SendError::MaxRetriesExceeded)
    }
}

/// Send errors
#[derive(Debug, Clone)]
pub enum SendError {
    NotConnected,
    BackPressured,
    AdminAction,
    AeronError(String),
    MaxRetriesExceeded,
    Unknown(i64),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotConnected => write!(f, "Not connected to subscriber"),
            SendError::BackPressured => write!(f, "Back pressure - subscriber slow"),
            SendError::AdminAction => write!(f, "Admin action in progress"),
            SendError::AeronError(e) => write!(f, "Aeron error: {}", e),
            SendError::MaxRetriesExceeded => write!(f, "Max retries exceeded"),
            SendError::Unknown(code) => write!(f, "Unknown error code: {}", code),
        }
    }
}

impl std::error::Error for SendError {}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Integration tests require running Aeron media driver
    // Unit tests only test error types

    #[test]
    fn test_send_error_display() {
        assert_eq!(format!("{}", SendError::NotConnected), "Not connected to subscriber");
        assert_eq!(format!("{}", SendError::BackPressured), "Back pressure - subscriber slow");
    }
}
