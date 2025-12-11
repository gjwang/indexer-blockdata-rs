// Metrics and monitoring for internal transfers
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Metrics collector for internal transfers
pub struct TransferMetrics {
    // Counters
    total_requests: AtomicU64,
    successful_transfers: AtomicU64,
    failed_transfers: AtomicU64,
    voided_transfers: AtomicU64,

    // Errors
    validation_errors: AtomicU64,
    tb_errors: AtomicU64,
    db_errors: AtomicU64,

    // Performance
    total_latency_ms: AtomicU64,
    max_latency_ms: AtomicU64,

    // Current state
    pending_count: AtomicU64,
    stuck_count: AtomicU64,
}

impl TransferMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            total_requests: AtomicU64::new(0),
            successful_transfers: AtomicU64::new(0),
            failed_transfers: AtomicU64::new(0),
            voided_transfers: AtomicU64::new(0),
            validation_errors: AtomicU64::new(0),
            tb_errors: AtomicU64::new(0),
            db_errors: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            max_latency_ms: AtomicU64::new(0),
            pending_count: AtomicU64::new(0),
            stuck_count: AtomicU64::new(0),
        })
    }

    pub fn record_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.successful_transfers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failed_transfers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_void(&self) {
        self.voided_transfers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_validation_error(&self) {
        self.validation_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tb_error(&self) {
        self.tb_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_db_error(&self) {
        self.db_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_latency(&self, latency_ms: u64) {
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);

        // Update max latency
        let mut current_max = self.max_latency_ms.load(Ordering::Relaxed);
        while latency_ms > current_max {
            match self.max_latency_ms.compare_exchange(
                current_max,
                latency_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(new_max) => current_max = new_max,
            }
        }
    }

    pub fn set_pending_count(&self, count: u64) {
        self.pending_count.store(count, Ordering::Relaxed);
    }

    pub fn set_stuck_count(&self, count: u64) {
        self.stuck_count.store(count, Ordering::Relaxed);
    }

    pub fn get_snapshot(&self) -> MetricsSnapshot {
        let total = self.total_requests.load(Ordering::Relaxed);
        let latency = self.total_latency_ms.load(Ordering::Relaxed);

        MetricsSnapshot {
            total_requests: total,
            successful_transfers: self.successful_transfers.load(Ordering::Relaxed),
            failed_transfers: self.failed_transfers.load(Ordering::Relaxed),
            voided_transfers: self.voided_transfers.load(Ordering::Relaxed),
            validation_errors: self.validation_errors.load(Ordering::Relaxed),
            tb_errors: self.tb_errors.load(Ordering::Relaxed),
            db_errors: self.db_errors.load(Ordering::Relaxed),
            avg_latency_ms: if total > 0 { latency / total } else { 0 },
            max_latency_ms: self.max_latency_ms.load(Ordering::Relaxed),
            pending_count: self.pending_count.load(Ordering::Relaxed),
            stuck_count: self.stuck_count.load(Ordering::Relaxed),
            success_rate: if total > 0 {
                (self.successful_transfers.load(Ordering::Relaxed) as f64 / total as f64) * 100.0
            } else {
                0.0
            },
        }
    }

    pub fn reset(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.successful_transfers.store(0, Ordering::Relaxed);
        self.failed_transfers.store(0, Ordering::Relaxed);
        self.voided_transfers.store(0, Ordering::Relaxed);
        self.validation_errors.store(0, Ordering::Relaxed);
        self.tb_errors.store(0, Ordering::Relaxed);
        self.db_errors.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
        self.max_latency_ms.store(0, Ordering::Relaxed);
    }
}

impl Default for TransferMetrics {
    fn default() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_transfers: AtomicU64::new(0),
            failed_transfers: AtomicU64::new(0),
            voided_transfers: AtomicU64::new(0),
            validation_errors: AtomicU64::new(0),
            tb_errors: AtomicU64::new(0),
            db_errors: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            max_latency_ms: AtomicU64::new(0),
            pending_count: AtomicU64::new(0),
            stuck_count: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub total_requests: u64,
    pub successful_transfers: u64,
    pub failed_transfers: u64,
    pub voided_transfers: u64,
    pub validation_errors: u64,
    pub tb_errors: u64,
    pub db_errors: u64,
    pub avg_latency_ms: u64,
    pub max_latency_ms: u64,
    pub pending_count: u64,
    pub stuck_count: u64,
    pub success_rate: f64,
}

impl MetricsSnapshot {
    pub fn to_prometheus(&self) -> String {
        format!(
            r#"# HELP internal_transfer_total_requests Total number of transfer requests
# TYPE internal_transfer_total_requests counter
internal_transfer_total_requests {}

# HELP internal_transfer_successful Total successful transfers
# TYPE internal_transfer_successful counter
internal_transfer_successful {}

# HELP internal_transfer_failed Total failed transfers
# TYPE internal_transfer_failed counter
internal_transfer_failed {}

# HELP internal_transfer_voided Total voided transfers
# TYPE internal_transfer_voided counter
internal_transfer_voided {}

# HELP internal_transfer_validation_errors Total validation errors
# TYPE internal_transfer_validation_errors counter
internal_transfer_validation_errors {}

# HELP internal_transfer_tb_errors TigerBeetle errors
# TYPE internal_transfer_tb_errors counter
internal_transfer_tb_errors {}

# HELP internal_transfer_db_errors Database errors
# TYPE internal_transfer_db_errors counter
internal_transfer_db_errors {}

# HELP internal_transfer_avg_latency_ms Average latency in milliseconds
# TYPE internal_transfer_avg_latency_ms gauge
internal_transfer_avg_latency_ms {}

# HELP internal_transfer_max_latency_ms Maximum latency in milliseconds
# TYPE internal_transfer_max_latency_ms gauge
internal_transfer_max_latency_ms {}

# HELP internal_transfer_pending_count Current pending transfers
# TYPE internal_transfer_pending_count gauge
internal_transfer_pending_count {}

# HELP internal_transfer_stuck_count Current stuck transfers
# TYPE internal_transfer_stuck_count gauge
internal_transfer_stuck_count {}

# HELP internal_transfer_success_rate Success rate percentage
# TYPE internal_transfer_success_rate gauge
internal_transfer_success_rate {}
"#,
            self.total_requests,
            self.successful_transfers,
            self.failed_transfers,
            self.voided_transfers,
            self.validation_errors,
            self.tb_errors,
            self.db_errors,
            self.avg_latency_ms,
            self.max_latency_ms,
            self.pending_count,
            self.stuck_count,
            self.success_rate
        )
    }
}

/// Timer for measuring operation latency
pub struct LatencyTimer {
    start: Instant,
}

impl LatencyTimer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recording() {
        let metrics = TransferMetrics::new();

        metrics.record_request();
        metrics.record_request();
        metrics.record_success();
        metrics.record_failure();

        let snapshot = metrics.get_snapshot();
        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.successful_transfers, 1);
        assert_eq!(snapshot.failed_transfers, 1);
        assert_eq!(snapshot.success_rate, 50.0);
    }

    #[test]
    fn test_latency_tracking() {
        let metrics = TransferMetrics::new();

        metrics.record_request();
        metrics.record_latency(100);
        metrics.record_request();
        metrics.record_latency(200);

        let snapshot = metrics.get_snapshot();
        assert_eq!(snapshot.avg_latency_ms, 150);
        assert_eq!(snapshot.max_latency_ms, 200);
    }

    #[test]
    fn test_prometheus_format() {
        let metrics = TransferMetrics::new();
        metrics.record_request();
        metrics.record_success();

        let snapshot = metrics.get_snapshot();
        let prom = snapshot.to_prometheus();

        assert!(prom.contains("internal_transfer_total_requests 1"));
        assert!(prom.contains("internal_transfer_successful 1"));
    }

    #[test]
    fn test_timer() {
        let timer = LatencyTimer::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = timer.elapsed_ms();
        assert!(elapsed >= 10);
    }
}
