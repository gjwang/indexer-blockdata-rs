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

pub mod aeron_config;
pub mod driver;
pub mod fill_receiver;
pub mod order_receiver;
pub mod order_sender;

pub use aeron_config::AeronConfig;
pub use driver::EmbeddedDriver;
pub use fill_receiver::{FillMessage, FillReceiver};
pub use order_receiver::{OrderMessage, OrderReceiver};
pub use order_sender::{OrderSender, SendError};
