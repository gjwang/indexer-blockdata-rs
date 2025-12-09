//! Communication module for UBSCore
//!
//! Uses Aeron for low-latency transport:
//! - aeron:udp?endpoint=host:port - UDP transport (works across servers)
//! - aeron:ipc - Shared memory (same machine only)
//!
//! Gateway ↔ UBSCore via Aeron UDP:
//! - Port 40456: Gateway → UBSCore (orders)
//! - Port 40457: UBSCore → Gateway (responses)
//!
//! # Architecture
//!
//! ```text
//! Gateway                              UBSCore
//!    │                                    │
//!    │── Publication ──► Subscription ───►│  (orders)
//!    │                                    │
//!    │◄── Subscription ◄── Publication ◄──│  (responses)
//!    │                                    │
//! ```

/// Trait for #[repr(C)] messages that can be converted to/from bytes
///
/// Implement this for zero-copy wire message serialization.
/// Safety: Only safe for #[repr(C)] structs with no padding issues.
pub trait WireMessage: Sized + Copy {
    /// Convert to byte slice (zero-copy)
    fn to_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }

    /// Parse from bytes
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < std::mem::size_of::<Self>() {
            return None;
        }
        unsafe {
            let ptr = bytes.as_ptr() as *const Self;
            Some(ptr.read())
        }
    }
}

pub mod aeron_channel;
pub mod aeron_config;
pub mod aeron_server;
pub mod driver;
pub mod fill_receiver;
pub mod gateway_client;
pub mod order_receiver;
pub mod order_sender;
pub mod response;
pub mod ubscore_handler;

pub use aeron_channel::{AeronChannel, AeronChannelConfig};
pub use aeron_config::AeronConfig;
pub use aeron_server::{AeronServer, AeronServerConfig, parse_request};
pub use driver::{EmbeddedDriver, AERON_DIR};
pub use fill_receiver::{FillMessage, FillReceiver};
pub use gateway_client::UbsGatewayClient;
pub use order_receiver::{OrderMessage, OrderReceiver};
pub use order_sender::{OrderSender, SendError};
pub use response::{ResponseMessage, reason_codes};
pub use ubscore_handler::{UbsCoreHandler, HandlerStats};
