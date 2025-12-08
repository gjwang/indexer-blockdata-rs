//! UBSCore - User Balance Service Core
//!
//! In-memory balance authority that validates orders before they reach the Matching Engine.
//! Target: ~50 µs end-to-end order entry latency

pub mod types;
pub mod error;
pub mod order;
pub mod dedup;
pub mod debt;
pub mod fee;
pub mod risk;
pub mod core;
pub mod wal;

// Re-exports
pub use error::RejectReason;
pub use order::InternalOrder;
pub use dedup::DeduplicationGuard;
pub use debt::{DebtLedger, DebtRecord, DebtReason};
pub use fee::VipFeeTable;
pub use risk::{RiskModel, SpotRiskModel};
pub use core::UBSCore;
pub use wal::{GroupCommitWal, WalEntry, WalEntryType, WalReplay};

