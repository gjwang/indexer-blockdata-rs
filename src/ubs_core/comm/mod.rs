//! Communication module for UBSCore
//!
//! Uses Aeron IPC for ultra-low latency (~100ns) communication:
//! - Gateway → UBSCore: Order requests
//! - UBSCore → Matching Engine: Validated orders
//! - Matching Engine → UBSCore: Trade fills
//!
//! # Quick Start (Embedded Driver for Dev)
//!
//! ```ignore
//! use fetcher::ubs_core::comm::{EmbeddedDriver, AeronConfig};
//!
//! // Launch embedded media driver
//! let _driver = EmbeddedDriver::launch()?;
//!
//! // Create receivers/senders
//! let config = AeronConfig::default();
//! let receiver = OrderReceiver::new(config.clone());
//! ```
//!
//! See docs/AERON_USAGE.md for full documentation.

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
