//! Communication module for UBSCore
//!
//! Supports two transport modes:
//! 1. UDP (default) - Simple, low-latency, works across servers
//! 2. Aeron IPC - Ultra-low latency (~100ns) for same-machine
//!
//! Gateway ↔ UBSCore via UDP:
//! - Port 40456: Gateway → UBSCore (orders)
//! - Port 40457: UBSCore → Gateway (responses)
//!
//! # Quick Start (UDP)
//!
//! ```ignore
//! // Gateway side
//! let client = UbsClient::new("ubs-host", 40456)?;
//! let response = client.send_order(&order)?;
//!
//! // UBSCore side
//! let server = UbsServer::new(40456)?;
//! let (req, addr) = server.recv_order()?;
//! server.send_response(addr, accept_response(...))?;
//! ```

pub mod aeron_config;
pub mod driver;
pub mod fill_receiver;
pub mod order_receiver;
pub mod order_sender;
pub mod ubs_client;
pub mod ubs_server;

pub use aeron_config::AeronConfig;
pub use driver::EmbeddedDriver;
pub use fill_receiver::{FillMessage, FillReceiver};
pub use order_receiver::{OrderMessage, OrderReceiver};
pub use order_sender::{OrderSender, SendError};
pub use ubs_client::{UbsClient, UbsResponse, OrderRequest, ResponseMessage};
pub use ubs_server::{UbsServer, accept_response, reject_response};
