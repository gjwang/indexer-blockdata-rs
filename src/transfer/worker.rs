//! Background Worker for Transfer Processing
//!
//! Provides sync processing with timeout and background scanner for stale transfers.

use crossbeam::queue::ArrayQueue;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::transfer::coordinator::TransferCoordinator;
use crate::transfer::db::TransferDb;
use crate::transfer::state::TransferState;

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
    buffer: ArrayQueue<Uuid>,
}

impl TransferQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: ArrayQueue::new(capacity),
        }
    }

    /// Try to push a req_id to the queue
    /// Returns false if queue is full (backpressure)
    pub fn try_push(&self, req_id: Uuid) -> bool {
        self.buffer.push(req_id).is_ok()
    }

    /// Try to pop a req_id from the queue
    pub fn try_pop(&self) -> Option<Uuid> {
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
    pub async fn process_now(&self, req_id: Uuid) -> TransferState {
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
    async fn process_transfer(&self, req_id: Uuid) {
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

    /// Run the background worker loop
    pub async fn run(&self) {
        log::info!("Transfer worker started (scan_interval={}ms, stale_after={}ms)",
            self.config.scan_interval_ms, self.config.stale_after_ms);

        loop {
            // 1. Process items from queue
            while let Some(req_id) = self.queue.try_pop() {
                self.process_transfer(req_id).await;
            }

            // 2. Scan for stale transfers
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

                        // EXPERT REVIEW: Rate limit to prevent thundering herd
                        sleep(Duration::from_millis(10)).await;
                    }
                }
                Err(e) => {
                    log::error!("Error scanning stale transfers: {}", e);
                }
            }

            // 3. Sleep before next scan
            sleep(Duration::from_millis(self.config.scan_interval_ms)).await;
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

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

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

        assert!(queue.try_push(Uuid::new_v4()));
        assert!(queue.try_push(Uuid::new_v4()));
        assert!(!queue.try_push(Uuid::new_v4())); // Full
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
