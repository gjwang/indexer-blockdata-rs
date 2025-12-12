//! Transfer module - main module file
//!
//! This module provides the internal transfer v2 implementation with FSM-based
//! processing, service adapters, and background worker.

pub mod state;
pub mod types;
pub mod db;
pub mod coordinator;
pub mod worker;
pub mod adapters;

// Re-export commonly used types
pub use state::TransferState;
pub use types::{OpResult, RequestId, ServiceId, TransferRecord, TransferRequest, TransferResponse};
pub use db::TransferDb;
pub use coordinator::TransferCoordinator;
pub use worker::{TransferWorker, TransferQueue, WorkerConfig};
