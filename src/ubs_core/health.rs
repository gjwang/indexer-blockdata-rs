//! Health checks for UBSCore
//!
//! Provides readiness and liveness probes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Health status
#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

impl HealthStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    pub fn is_ready(&self) -> bool {
        !matches!(self, HealthStatus::Unhealthy(_))
    }
}

/// Health checker for UBSCore
pub struct HealthChecker {
    /// Is the service ready to accept requests
    ready: AtomicBool,

    /// Last successful heartbeat timestamp (epoch ms)
    last_heartbeat_ms: AtomicU64,

    /// Maximum time since last heartbeat before considered unhealthy
    heartbeat_timeout: Duration,

    /// Maximum pending queue size before degraded
    max_pending_queue: usize,

    /// Current pending queue size
    pending_queue_size: AtomicU64,

    /// Start time
    start_time: Instant,
}

impl HealthChecker {
    pub fn new(heartbeat_timeout: Duration, max_pending_queue: usize) -> Self {
        Self {
            ready: AtomicBool::new(false),
            last_heartbeat_ms: AtomicU64::new(0),
            heartbeat_timeout,
            max_pending_queue,
            pending_queue_size: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Set ready state
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    /// Record a heartbeat
    pub fn heartbeat(&self) {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;
        self.last_heartbeat_ms.store(now, Ordering::SeqCst);
    }

    /// Update pending queue size
    pub fn set_pending_queue_size(&self, size: usize) {
        self.pending_queue_size.store(size as u64, Ordering::Relaxed);
    }

    /// Check liveness (is the process alive and functional)
    pub fn liveness(&self) -> HealthStatus {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;

        let last = self.last_heartbeat_ms.load(Ordering::SeqCst);

        // First heartbeat not yet received - check if we're past startup grace period
        if last == 0 {
            if self.start_time.elapsed() < Duration::from_secs(30) {
                return HealthStatus::Degraded("Starting up".into());
            } else {
                return HealthStatus::Unhealthy("No heartbeat received".into());
            }
        }

        let elapsed = Duration::from_millis(now - last);
        if elapsed > self.heartbeat_timeout {
            return HealthStatus::Unhealthy(format!(
                "Heartbeat timeout: {}ms since last heartbeat",
                elapsed.as_millis()
            ));
        }

        HealthStatus::Healthy
    }

    /// Check readiness (can accept new requests)
    pub fn readiness(&self) -> HealthStatus {
        if !self.ready.load(Ordering::SeqCst) {
            return HealthStatus::Unhealthy("Not ready".into());
        }

        let liveness = self.liveness();
        if !liveness.is_healthy() {
            return liveness;
        }

        let pending = self.pending_queue_size.load(Ordering::Relaxed);
        if pending > self.max_pending_queue as u64 {
            return HealthStatus::Degraded(format!(
                "High queue depth: {} (max {})",
                pending, self.max_pending_queue
            ));
        }

        // Warn at 80% capacity
        if pending > (self.max_pending_queue as u64 * 8 / 10) {
            return HealthStatus::Degraded(format!(
                "Queue near capacity: {} / {}",
                pending, self.max_pending_queue
            ));
        }

        HealthStatus::Healthy
    }

    /// Get uptime
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get health report as JSON
    pub fn health_report(&self) -> String {
        let liveness = self.liveness();
        let readiness = self.readiness();

        format!(
            r#"{{"liveness":"{}","readiness":"{}","uptime_secs":{},"pending_queue":{}}}"#,
            match &liveness {
                HealthStatus::Healthy => "healthy",
                HealthStatus::Degraded(_) => "degraded",
                HealthStatus::Unhealthy(_) => "unhealthy",
            },
            match &readiness {
                HealthStatus::Healthy => "healthy",
                HealthStatus::Degraded(_) => "degraded",
                HealthStatus::Unhealthy(_) => "unhealthy",
            },
            self.uptime().as_secs(),
            self.pending_queue_size.load(Ordering::Relaxed)
        )
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new(Duration::from_secs(30), 10_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_not_ready() {
        let health = HealthChecker::default();
        assert!(!health.readiness().is_ready());
    }

    #[test]
    fn test_health_ready() {
        let health = HealthChecker::default();
        health.set_ready(true);
        health.heartbeat();

        assert!(health.liveness().is_healthy());
        assert!(health.readiness().is_ready());
    }

    #[test]
    fn test_health_degraded_queue() {
        let health = HealthChecker::new(Duration::from_secs(30), 100);
        health.set_ready(true);
        health.heartbeat();
        health.set_pending_queue_size(90); // 90% capacity

        let status = health.readiness();
        assert!(matches!(status, HealthStatus::Degraded(_)));
    }

    #[test]
    fn test_health_report() {
        let health = HealthChecker::default();
        health.set_ready(true);
        health.heartbeat();

        let report = health.health_report();
        assert!(report.contains("liveness"));
        assert!(report.contains("readiness"));
    }
}
