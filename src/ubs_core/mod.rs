//! UBSCore - User Balance Service Core
//!
//! In-memory balance authority that validates orders before they reach the Matching Engine.
//! Target: ~50 µs end-to-end order entry latency

#[cfg(feature = "aeron")]
pub mod comm;
pub mod core;
pub mod debt;
pub mod dedup;
pub mod error;
pub mod fee;
pub mod health;
pub mod metrics;
pub mod order;
pub mod risk;
pub mod types;
pub mod wal;

#[cfg(test)]
mod bench;

// Re-exports
#[cfg(feature = "aeron")]
pub use comm::{AeronConfig, FillReceiver, OrderReceiver, OrderSender};
pub use core::{OrderMessage, UBSCore};
pub use debt::{DebtLedger, DebtReason, DebtRecord};
pub use dedup::DeduplicationGuard;
pub use error::RejectReason;
pub use fee::VipFeeTable;
pub use health::{HealthChecker, HealthStatus};
pub use metrics::{LatencyTimer, MetricsSnapshot, OrderMetrics};
pub use order::{InternalOrder, OrderType, Side};
pub use risk::{RiskModel, SpotRiskModel};
pub use wal::{GroupCommitConfig, GroupCommitWal, MmapWal, WalEntry, WalEntryType, WalReplay, install_sigbus_handler};
