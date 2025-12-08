//! Metrics for UBSCore
//!
//! Provides counters and gauges for monitoring:
//! - Order processing latency
//! - Order counts by status
//! - Balance operations
//! - WAL statistics

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Metrics for order processing
#[derive(Default)]
pub struct OrderMetrics {
    /// Total orders received
    pub orders_received: AtomicU64,

    /// Orders accepted (passed all checks)
    pub orders_accepted: AtomicU64,

    /// Orders rejected by reason
    pub rejected_dedup: AtomicU64,
    pub rejected_balance: AtomicU64,
    pub rejected_risk: AtomicU64,
    pub rejected_overflow: AtomicU64,
    pub rejected_account_not_found: AtomicU64,
    pub rejected_system_busy: AtomicU64,
    pub rejected_other: AtomicU64,

    /// Latency tracking (nanoseconds)
    pub total_latency_ns: AtomicU64,
    pub min_latency_ns: AtomicU64,
    pub max_latency_ns: AtomicU64,
}

impl OrderMetrics {
    pub fn new() -> Self {
        Self { min_latency_ns: AtomicU64::new(u64::MAX), ..Default::default() }
    }

    /// Record order received
    pub fn record_received(&self) {
        self.orders_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Record order accepted
    pub fn record_accepted(&self, latency_ns: u64) {
        self.orders_accepted.fetch_add(1, Ordering::Relaxed);
        self.record_latency(latency_ns);
    }

    /// Record order rejected by reason
    pub fn record_rejected(&self, reason: &str, latency_ns: u64) {
        match reason {
            "DuplicateOrderId" => self.rejected_dedup.fetch_add(1, Ordering::Relaxed),
            "InsufficientBalance" => self.rejected_balance.fetch_add(1, Ordering::Relaxed),
            "RiskCheckFailed" => self.rejected_risk.fetch_add(1, Ordering::Relaxed),
            "OrderCostOverflow" => self.rejected_overflow.fetch_add(1, Ordering::Relaxed),
            "AccountNotFound" => self.rejected_account_not_found.fetch_add(1, Ordering::Relaxed),
            "SystemBusy" => self.rejected_system_busy.fetch_add(1, Ordering::Relaxed),
            _ => self.rejected_other.fetch_add(1, Ordering::Relaxed),
        };
        self.record_latency(latency_ns);
    }

    fn record_latency(&self, latency_ns: u64) {
        self.total_latency_ns.fetch_add(latency_ns, Ordering::Relaxed);

        // Update min (CAS loop)
        loop {
            let current = self.min_latency_ns.load(Ordering::Relaxed);
            if latency_ns >= current {
                break;
            }
            if self
                .min_latency_ns
                .compare_exchange_weak(current, latency_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        // Update max (CAS loop)
        loop {
            let current = self.max_latency_ns.load(Ordering::Relaxed);
            if latency_ns <= current {
                break;
            }
            if self
                .max_latency_ns
                .compare_exchange_weak(current, latency_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }

    /// Get average latency in nanoseconds
    pub fn avg_latency_ns(&self) -> u64 {
        let total = self.total_latency_ns.load(Ordering::Relaxed);
        let count =
            self.orders_accepted.load(Ordering::Relaxed) + self.total_rejected(Ordering::Relaxed);
        if count == 0 {
            0
        } else {
            total / count
        }
    }

    /// Get total rejected count
    pub fn total_rejected(&self, order: Ordering) -> u64 {
        self.rejected_dedup.load(order)
            + self.rejected_balance.load(order)
            + self.rejected_risk.load(order)
            + self.rejected_overflow.load(order)
            + self.rejected_account_not_found.load(order)
            + self.rejected_system_busy.load(order)
            + self.rejected_other.load(order)
    }

    /// Get snapshot of all metrics
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            orders_received: self.orders_received.load(Ordering::Relaxed),
            orders_accepted: self.orders_accepted.load(Ordering::Relaxed),
            orders_rejected: self.total_rejected(Ordering::Relaxed),
            avg_latency_us: self.avg_latency_ns() / 1000,
            min_latency_us: {
                let min = self.min_latency_ns.load(Ordering::Relaxed);
                if min == u64::MAX {
                    0
                } else {
                    min / 1000
                }
            },
            max_latency_us: self.max_latency_ns.load(Ordering::Relaxed) / 1000,
        }
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.orders_received.store(0, Ordering::Relaxed);
        self.orders_accepted.store(0, Ordering::Relaxed);
        self.rejected_dedup.store(0, Ordering::Relaxed);
        self.rejected_balance.store(0, Ordering::Relaxed);
        self.rejected_risk.store(0, Ordering::Relaxed);
        self.rejected_overflow.store(0, Ordering::Relaxed);
        self.rejected_account_not_found.store(0, Ordering::Relaxed);
        self.rejected_system_busy.store(0, Ordering::Relaxed);
        self.rejected_other.store(0, Ordering::Relaxed);
        self.total_latency_ns.store(0, Ordering::Relaxed);
        self.min_latency_ns.store(u64::MAX, Ordering::Relaxed);
        self.max_latency_ns.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of metrics at a point in time
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub orders_received: u64,
    pub orders_accepted: u64,
    pub orders_rejected: u64,
    pub avg_latency_us: u64,
    pub min_latency_us: u64,
    pub max_latency_us: u64,
}

impl MetricsSnapshot {
    /// Format as JSON for logging/export
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"orders_received":{},"orders_accepted":{},"orders_rejected":{},"avg_latency_us":{},"min_latency_us":{},"max_latency_us":{}}}"#,
            self.orders_received,
            self.orders_accepted,
            self.orders_rejected,
            self.avg_latency_us,
            self.min_latency_us,
            self.max_latency_us
        )
    }

    /// Calculate throughput (orders per second)
    pub fn throughput(&self, duration_secs: f64) -> f64 {
        if duration_secs <= 0.0 {
            return 0.0;
        }
        self.orders_received as f64 / duration_secs
    }
}

/// Timer for measuring operation latency
pub struct LatencyTimer {
    start: Instant,
}

impl LatencyTimer {
    pub fn start() -> Self {
        Self { start: Instant::now() }
    }

    /// Get elapsed time in nanoseconds
    pub fn elapsed_ns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    /// Get elapsed time in microseconds
    pub fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_metrics_basic() {
        let metrics = OrderMetrics::new();

        metrics.record_received();
        metrics.record_received();
        metrics.record_accepted(1000);
        metrics.record_rejected("DuplicateOrderId", 500);

        assert_eq!(metrics.orders_received.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.orders_accepted.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.rejected_dedup.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_latency_tracking() {
        let metrics = OrderMetrics::new();

        metrics.record_accepted(1000);
        metrics.record_accepted(2000);
        metrics.record_accepted(3000);

        assert_eq!(metrics.min_latency_ns.load(Ordering::Relaxed), 1000);
        assert_eq!(metrics.max_latency_ns.load(Ordering::Relaxed), 3000);
        assert_eq!(metrics.avg_latency_ns(), 2000);
    }

    #[test]
    fn test_snapshot() {
        let metrics = OrderMetrics::new();

        metrics.record_received();
        metrics.record_accepted(1000_000); // 1ms

        let snap = metrics.snapshot();
        assert_eq!(snap.orders_received, 1);
        assert_eq!(snap.orders_accepted, 1);
        assert_eq!(snap.avg_latency_us, 1000);
    }

    #[test]
    fn test_latency_timer() {
        let timer = LatencyTimer::start();
        std::thread::sleep(std::time::Duration::from_micros(100));
        let elapsed = timer.elapsed_us();
        assert!(elapsed >= 100);
    }
}
