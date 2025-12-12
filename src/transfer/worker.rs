//! Background Worker for Transfer Processing
//!
//! Provides sync processing with timeout and background scanner for stale transfers.

use crossbeam::queue::ArrayQueue;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::transfer::coordinator::TransferCoordinator;
use crate::transfer::db::TransferDb;
use crate::transfer::state::TransferState;
use crate::transfer::types::RequestId;

/// Worker configuration
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Scan interval for stale transfers (ms)
    pub scan_interval_ms: u64,
    /// Timeout for sync processing (ms)
    pub process_timeout_ms: u64,
    /// Stale threshold (ms) - transfers older than this are picked up by scanner
    pub stale_after_ms: i64,
    /// Alert after this many retries
    pub alert_threshold: u32,
    /// Queue capacity
    pub queue_capacity: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            scan_interval_ms: 5000,     // 5 seconds
            process_timeout_ms: 2000,   // 2 seconds for sync processing
            stale_after_ms: 30000,      // 30 seconds
            alert_threshold: 10,
            queue_capacity: 10000,
        }
    }
}

/// Transfer queue (ring buffer with backpressure)
pub struct TransferQueue {
    buffer: ArrayQueue<RequestId>,
}

impl TransferQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: ArrayQueue::new(capacity),
        }
    }

    /// Try to push a req_id to the queue
    /// Returns false if queue is full (backpressure)
    pub fn try_push(&self, req_id: RequestId) -> bool {
        self.buffer.push(req_id).is_ok()
    }

    /// Try to pop a req_id from the queue
    pub fn try_pop(&self) -> Option<RequestId> {
        self.buffer.pop()
    }

    /// Get current queue length
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Background worker for transfer processing
pub struct TransferWorker {
    coordinator: Arc<TransferCoordinator>,
    db: Arc<TransferDb>,
    queue: Arc<TransferQueue>,
    config: WorkerConfig,
}

impl TransferWorker {
    pub fn new(
        coordinator: Arc<TransferCoordinator>,
        db: Arc<TransferDb>,
        queue: Arc<TransferQueue>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            coordinator,
            db,
            queue,
            config,
        }
    }

    /// Process a transfer synchronously with timeout
    ///
    /// This is the "happy path optimization" - attempt to complete
    /// the transfer immediately. If it doesn't complete in time,
    /// return the current state and let the background worker continue.
    pub async fn process_now(&self, req_id: RequestId) -> TransferState {
        let deadline = Instant::now() + Duration::from_millis(self.config.process_timeout_ms);

        loop {
            match self.coordinator.step(req_id).await {
                Ok(state) => {
                    if state.is_terminal() {
                        return state;
                    }
                    if Instant::now() >= deadline {
                        // Timeout - return current state, background worker will continue
                        return state;
                    }
                    // Small delay between steps
                    sleep(Duration::from_millis(10)).await;
                }
                Err(e) => {
                    log::error!("Error processing transfer {}: {}", req_id, e);
                    // Return current state from DB
                    return self.db.get(req_id).await
                        .ok()
                        .flatten()
                        .map(|r| r.state)
                        .unwrap_or(TransferState::Init);
                }
            }
        }
    }

    /// Process a transfer in the background (no timeout)
    async fn process_transfer(&self, req_id: RequestId) {
        loop {
            match self.coordinator.step(req_id).await {
                Ok(state) => {
                    if state.is_terminal() {
                        log::info!("Transfer {} completed: {:?}", req_id, state);
                        return;
                    }
                    // Small delay between steps
                    sleep(Duration::from_millis(100)).await;
                }
                Err(e) => {
                    log::error!("Error processing transfer {}: {}", req_id, e);
                    return;
                }
            }
        }
    }

    /// Run the background worker loop with graceful shutdown
    pub async fn run_with_shutdown(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        log::info!("Transfer worker started with shutdown signal (scan_interval={}ms, stale_after={}ms)",
            self.config.scan_interval_ms, self.config.stale_after_ms);

        loop {
            tokio::select! {
                biased;

                _ = shutdown.changed() => {
                    log::info!("Transfer worker received shutdown signal");
                    break;
                }

                _ = self.run_one_cycle() => {}
            }
        }

        log::info!("Transfer worker stopped");
    }

    /// Run one cycle of the worker loop
    async fn run_one_cycle(&self) {
        // 1. Process items from queue
        while let Some(req_id) = self.queue.try_pop() {
            self.process_transfer(req_id).await;
        }

        // 2. Scan for stale transfers
        self.scan_stale_transfers().await;

        // 3. Sleep before next scan
        sleep(Duration::from_millis(self.config.scan_interval_ms)).await;
    }

    /// Run the background worker loop (no graceful shutdown)
    pub async fn run(&self) {
        log::info!("Transfer worker started (scan_interval={}ms, stale_after={}ms)",
            self.config.scan_interval_ms, self.config.stale_after_ms);

        loop {
            self.run_one_cycle().await;
        }
    }

    /// Scan and process stale transfers
    async fn scan_stale_transfers(&self) {
        match self.db.find_stale(self.config.stale_after_ms).await {
            Ok(stale) => {
                if !stale.is_empty() {
                    log::info!("Found {} stale transfers", stale.len());
                }

                for record in stale {
                    log::info!(
                        "Processing stale transfer: {} (state: {:?}, retries: {})",
                        record.req_id, record.state, record.retry_count
                    );

                    // Check alert threshold
                    if record.retry_count >= self.config.alert_threshold {
                        log::warn!(
                            "ALERT: Transfer {} stuck after {} retries (state: {:?})",
                            record.req_id, record.retry_count, record.state
                        );
                        // TODO: Send alert to monitoring system
                    }

                    self.process_transfer(record.req_id).await;

                    // Exponential backoff: 10ms * 2^retries, capped at 5s
                    let backoff_ms = std::cmp::min(
                        10 * 2u64.saturating_pow(record.retry_count),
                        5000,
                    );
                    sleep(Duration::from_millis(backoff_ms)).await;
                }
            }
            Err(e) => {
                log::error!("Error scanning stale transfers: {}", e);
            }
        }
    }

    /// Start the worker in a background task
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_queue() {
        let queue = TransferQueue::new(100);

        let id1 = RequestId::new(1001);
        let id2 = RequestId::new(1002);

        assert!(queue.try_push(id1));
        assert!(queue.try_push(id2));
        assert_eq!(queue.len(), 2);

        assert_eq!(queue.try_pop(), Some(id1));
        assert_eq!(queue.try_pop(), Some(id2));
        assert_eq!(queue.try_pop(), None);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_queue_backpressure() {
        let queue = TransferQueue::new(2);

        assert!(queue.try_push(RequestId::new(1)));
        assert!(queue.try_push(RequestId::new(2)));
        assert!(!queue.try_push(RequestId::new(3))); // Full
    }

    #[test]
    fn test_worker_config_defaults() {
        let config = WorkerConfig::default();

        assert_eq!(config.scan_interval_ms, 5000);
        assert_eq!(config.process_timeout_ms, 2000);
        assert_eq!(config.stale_after_ms, 30000);
        assert_eq!(config.alert_threshold, 10);
    }
}
