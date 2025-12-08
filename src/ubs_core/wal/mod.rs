//! WAL (Write-Ahead Log) module for UBSCore
//!
//! Provides crash-safe persistence with:
//! - O_DIRECT aligned writes
//! - CRC32 checksums
//! - Group commit batching
//! - Replay for recovery

pub mod aligned_buffer;
pub mod entry;
pub mod group_commit;
pub mod replay;

pub use aligned_buffer::AlignedBuffer;
pub use entry::{WalEntry, WalEntryType};
pub use group_commit::GroupCommitWal;
pub use replay::WalReplay;
