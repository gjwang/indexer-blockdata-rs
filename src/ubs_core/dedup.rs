//! Deduplication guard for order IDs
//!
//! Uses IndexSet for O(1) lookup + ordered eviction
//! Order IDs are u64 in SnowflakeGenRng format:
//!   - 44 bits: timestamp (ms)
//!   - 7 bits: machine ID
//!   - 13 bits: sequence

use super::error::RejectReason;
use crate::fast_ulid::SnowflakeGenRng;
use indexmap::IndexSet;

const CACHE_SIZE: usize = 10_000;
const MAX_TIME_DRIFT_MS: u64 = 3_000;

/// Guard against duplicate order IDs
pub struct DeduplicationGuard {
    cache: IndexSet<u64>,
    min_allowed_ts: u64,
}

impl DeduplicationGuard {
    pub fn new() -> Self {
        Self { cache: IndexSet::with_capacity(CACHE_SIZE), min_allowed_ts: 0 }
    }

    /// Check and record order ID
    /// order_id is a u64 in SnowflakeGenRng format
    /// Returns Ok if new, Err if duplicate or too old
    pub fn check_and_record(&mut self, order_id: u64, now: u64) -> Result<(), RejectReason> {
        let order_ts = SnowflakeGenRng::timestamp_ms(order_id);

        // 1. TIME CHECK: too old
        if now.saturating_sub(order_ts) > MAX_TIME_DRIFT_MS {
            return Err(RejectReason::OrderTooOld);
        }

        // 2. TIME CHECK: future timestamp
        if order_ts > now.saturating_add(1_000) {
            return Err(RejectReason::FutureTimestamp);
        }

        // 3. TIME CHECK: older than evicted entries
        if order_ts < self.min_allowed_ts {
            return Err(RejectReason::OrderTooOld);
        }

        // 4. DUPLICATE CHECK
        if self.cache.contains(&order_id) {
            return Err(RejectReason::DuplicateOrderId);
        }

        // 5. EVICT IF FULL
        if self.cache.len() >= CACHE_SIZE {
            if let Some(evicted) = self.cache.pop() {
                self.min_allowed_ts =
                    self.min_allowed_ts.max(SnowflakeGenRng::timestamp_ms(evicted));
            }
        }

        // 6. INSERT
        self.cache.insert(order_id);
        Ok(())
    }
}

impl Default for DeduplicationGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accept_valid_order() {
        let mut guard = DeduplicationGuard::new();
        let order_id = SnowflakeGenRng::from_parts(1000, 1, 0);
        assert!(guard.check_and_record(order_id, 1000).is_ok());
    }

    #[test]
    fn test_reject_duplicate() {
        let mut guard = DeduplicationGuard::new();
        let order_id = SnowflakeGenRng::from_parts(1000, 1, 0);
        guard.check_and_record(order_id, 1000).unwrap();
        assert_eq!(guard.check_and_record(order_id, 1000), Err(RejectReason::DuplicateOrderId));
    }

    #[test]
    fn test_reject_too_old() {
        let mut guard = DeduplicationGuard::new();
        let order_id = SnowflakeGenRng::from_parts(1000, 1, 0);
        assert_eq!(
            guard.check_and_record(order_id, 5000), // 4 seconds later
            Err(RejectReason::OrderTooOld)
        );
    }

    #[test]
    fn test_reject_future() {
        let mut guard = DeduplicationGuard::new();
        let order_id = SnowflakeGenRng::from_parts(5000, 1, 0);
        assert_eq!(
            guard.check_and_record(order_id, 1000), // 4 seconds in future
            Err(RejectReason::FutureTimestamp)
        );
    }
}
