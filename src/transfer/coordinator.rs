//! Transfer Coordinator
//!
//! Orchestrates the FSM-based transfer processing.

use anyhow::Result;
use std::sync::Arc;
use uuid::Uuid;

use crate::transfer::adapters::ServiceAdapter;
use crate::transfer::db::TransferDb;
use crate::transfer::state::TransferState;
use crate::transfer::types::{OpResult, ServiceId, TransferRecord, TransferRequest};

/// Transfer Coordinator - orchestrates FSM-based processing
pub struct TransferCoordinator {
    db: Arc<TransferDb>,
    funding_adapter: Arc<dyn ServiceAdapter>,
    trading_adapter: Arc<dyn ServiceAdapter>,
}

impl TransferCoordinator {
    pub fn new(
        db: Arc<TransferDb>,
        funding_adapter: Arc<dyn ServiceAdapter>,
        trading_adapter: Arc<dyn ServiceAdapter>,
    ) -> Self {
        Self {
            db,
            funding_adapter,
            trading_adapter,
        }
    }

    /// Create a new transfer record
    pub async fn create(&self, req: TransferRequest) -> Result<Uuid> {
        // Validate request
        if req.amount == 0 {
            return Err(anyhow::anyhow!("Amount must be greater than 0"));
        }

        let source = ServiceId::from_str(&req.from)
            .ok_or_else(|| anyhow::anyhow!("Invalid source: {}", req.from))?;
        let target = ServiceId::from_str(&req.to)
            .ok_or_else(|| anyhow::anyhow!("Invalid target: {}", req.to))?;

        if source == target {
            return Err(anyhow::anyhow!("Source and target cannot be the same"));
        }

        let req_id = Uuid::new_v4();
        let now = chrono::Utc::now().timestamp_millis();

        let record = TransferRecord {
            req_id,
            source,
            target,
            user_id: req.user_id,
            asset_id: req.asset_id,
            amount: req.amount,
            state: TransferState::Init,
            created_at: now,
            updated_at: now,
            error: None,
            retry_count: 0,
        };

        self.db.create(&record).await?;
        log::info!("Created transfer: {} ({:?} -> {:?})", req_id, source, target);

        Ok(req_id)
    }

    /// Execute one step of the FSM
    /// Returns the new state after processing
    pub async fn step(&self, req_id: Uuid) -> Result<TransferState> {
        let record = self
            .db
            .get(req_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Transfer not found: {}", req_id))?;

        // Already terminal - nothing to do
        if record.state.is_terminal() {
            return Ok(record.state);
        }

        // Get adapters for source and target
        let source = self.get_adapter(record.source);
        let target = self.get_adapter(record.target);

        // Process based on current state
        let new_state = match record.state {
            TransferState::Init => {
                self.step_init(&record, source.as_ref()).await?
            }
            TransferState::SourcePending => {
                self.step_source_pending(&record, source.as_ref()).await?
            }
            TransferState::SourceDone => {
                self.step_source_done(&record, source.as_ref(), target.as_ref()).await?
            }
            TransferState::TargetPending => {
                self.step_target_pending(&record, source.as_ref(), target.as_ref()).await?
            }
            TransferState::Compensating => {
                self.step_compensating(&record, source.as_ref()).await?
            }
            _ => record.state, // Terminal states
        };

        // Increment retry count
        if !new_state.is_terminal() && new_state == record.state {
            self.db.increment_retry(req_id).await?;
        }

        Ok(new_state)
    }

    fn get_adapter(&self, service: ServiceId) -> Arc<dyn ServiceAdapter> {
        match service {
            ServiceId::Funding => self.funding_adapter.clone(),
            ServiceId::Trading => self.trading_adapter.clone(),
        }
    }

    /// Helper: Finalize source commit after target success
    /// Logs warning if commit fails but does not fail the transfer
    async fn finalize_source_commit(&self, record: &TransferRecord, source: &dyn ServiceAdapter) {
        let commit_result = source.commit(record.req_id).await;
        if let OpResult::Failed(e) = &commit_result {
            log::warn!(
                "Source commit failed for {} (target already received funds): {}",
                record.req_id, e
            );
            // TODO: Send alert to ops for manual cleanup of frozen funds
        }
    }

    /// Step from Init state: Call source.withdraw()
    async fn step_init(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        // 1. Persist SourcePending BEFORE calling service (persist-before-call)
        if !self.db.update_state_if(record.req_id, TransferState::Init, TransferState::SourcePending).await? {
            // Another worker already transitioned - get current state
            return Ok(self.db.get(record.req_id).await?.map(|r| r.state).unwrap_or(TransferState::Init));
        }

        // 2. Call source withdraw
        let result = source.withdraw(
            record.req_id,
            record.user_id,
            record.asset_id,
            record.amount,
        ).await;

        // 3. Handle result
        match result {
            OpResult::Success => {
                self.db.update_state_if(record.req_id, TransferState::SourcePending, TransferState::SourceDone).await?;
                Ok(TransferState::SourceDone)
            }
            OpResult::Failed(e) => {
                self.db.update_state_with_error(record.req_id, TransferState::SourcePending, TransferState::Failed, &e).await?;
                Ok(TransferState::Failed)
            }
            OpResult::Pending => {
                // Stay in SourcePending, will retry on next scan
                Ok(TransferState::SourcePending)
            }
        }
    }

    /// Step from SourcePending state: Re-call source.withdraw() (idempotent)
    async fn step_source_pending(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        // Query or re-call source (idempotent)
        let result = source.withdraw(
            record.req_id,
            record.user_id,
            record.asset_id,
            record.amount,
        ).await;

        match result {
            OpResult::Success => {
                self.db.update_state_if(record.req_id, TransferState::SourcePending, TransferState::SourceDone).await?;
                Ok(TransferState::SourceDone)
            }
            OpResult::Failed(e) => {
                self.db.update_state_with_error(record.req_id, TransferState::SourcePending, TransferState::Failed, &e).await?;
                Ok(TransferState::Failed)
            }
            OpResult::Pending => {
                Ok(TransferState::SourcePending)
            }
        }
    }

    /// Step from SourceDone state: Call target.deposit()
    async fn step_source_done(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
        target: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        // 1. Persist TargetPending BEFORE calling service
        if !self.db.update_state_if(record.req_id, TransferState::SourceDone, TransferState::TargetPending).await? {
            return Ok(self.db.get(record.req_id).await?.map(|r| r.state).unwrap_or(TransferState::SourceDone));
        }

        // 2. Call target deposit
        let result = target.deposit(
            record.req_id,
            record.user_id,
            record.asset_id,
            record.amount,
        ).await;

        // 3. Handle result
        match result {
            OpResult::Success => {
                // Finalize source commit
                self.finalize_source_commit(record, source).await;
                self.db.update_state_if(record.req_id, TransferState::TargetPending, TransferState::Committed).await?;
                Ok(TransferState::Committed)
            }
            OpResult::Failed(e) => {
                self.db.update_state_with_error(record.req_id, TransferState::TargetPending, TransferState::Compensating, &e).await?;
                Ok(TransferState::Compensating)
            }
            OpResult::Pending => {
                Ok(TransferState::TargetPending)
            }
        }
    }

    /// Step from TargetPending state: Re-call target.deposit() (idempotent)
    async fn step_target_pending(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
        target: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        let result = target.deposit(
            record.req_id,
            record.user_id,
            record.asset_id,
            record.amount,
        ).await;

        match result {
            OpResult::Success => {
                // Finalize source commit
                self.finalize_source_commit(record, source).await;
                self.db.update_state_if(record.req_id, TransferState::TargetPending, TransferState::Committed).await?;
                Ok(TransferState::Committed)
            }
            OpResult::Failed(e) => {
                self.db.update_state_with_error(record.req_id, TransferState::TargetPending, TransferState::Compensating, &e).await?;
                Ok(TransferState::Compensating)
            }
            OpResult::Pending => {
                Ok(TransferState::TargetPending)
            }
        }
    }

    /// Step from Compensating state: Call source.rollback()
    async fn step_compensating(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        let result = source.rollback(record.req_id).await;

        match result {
            OpResult::Success => {
                self.db.update_state_if(record.req_id, TransferState::Compensating, TransferState::RolledBack).await?;
                Ok(TransferState::RolledBack)
            }
            OpResult::Failed(e) => {
                // Rollback failed - stay in Compensating, keep retrying
                log::warn!("Rollback failed for {}: {} (will retry)", record.req_id, e);
                Ok(TransferState::Compensating)
            }
            OpResult::Pending => {
                Ok(TransferState::Compensating)
            }
        }
    }

    /// Get current state of a transfer
    pub async fn get_state(&self, req_id: Uuid) -> Result<Option<TransferState>> {
        Ok(self.db.get(req_id).await?.map(|r| r.state))
    }

    /// Get full transfer record
    pub async fn get(&self, req_id: Uuid) -> Result<Option<TransferRecord>> {
        self.db.get(req_id).await
    }
}
