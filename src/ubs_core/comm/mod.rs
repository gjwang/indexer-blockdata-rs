//! Communication module for UBSCore
//!
//! Uses Aeron IPC for ultra-low latency (~100ns) communication:
//! - Gateway → UBSCore: Order requests
//! - UBSCore → Matching Engine: Validated orders
//! - Matching Engine → UBSCore: Trade fills

pub mod aeron_config;
pub mod fill_receiver;
pub mod order_receiver;
pub mod order_sender;

pub use aeron_config::AeronConfig;
pub use fill_receiver::FillReceiver;
pub use order_receiver::OrderReceiver;
pub use order_sender::OrderSender;
