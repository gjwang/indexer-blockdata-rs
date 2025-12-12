# Internal Transfer - Implementation Plan

**Version**: 1.0
**Date**: 2025-12-12
**Status**: Ready for Implementation
**Estimated Time**: 15-18 hours

---

## Phase Overview

| Phase | Description | Time | Dependencies |
|-------|-------------|------|--------------|
| 1 | Database Schema & Types | 2h | None |
| 2 | FSM & State Machine | 2h | Phase 1 |
| 3 | DB Operations | 2h | Phase 1,2 |
| 4 | Service Adapter Trait & Mocks | 2h | Phase 2 |
| 5 | Coordinator | 2h | Phase 3,4 |
| 6 | Background Worker | 2h | Phase 5 |
| 7 | Gateway Integration | 2h | Phase 5,6 |
| 8 | E2E Testing | 3h | All |

---

## Phase 1: Database Schema & Types (2 hours)

### 1.1 Create Database Schema

**File**: `schema/transfer.cql`

```sql
-- Internal Transfer Table
CREATE TABLE IF NOT EXISTS trading.transfers (
    req_id uuid,
    source text,           -- 'funding' or 'trading'
    target text,           -- 'funding' or 'trading'
    user_id bigint,
    asset_id int,
    amount bigint,
    state text,            -- FSM state
    created_at bigint,
    updated_at bigint,
    error text,
    retry_count int,
    PRIMARY KEY (req_id)
);

-- Note: For scanning stale transfers, we'll use ALLOW FILTERING
-- In production, consider materialized view or secondary index if needed
```

### 1.2 Create Module Structure

```
src/transfer/
├── mod.rs
├── types.rs
├── state.rs
├── db.rs
├── coordinator.rs
├── worker.rs
└── adapters/
    ├── mod.rs
    ├── traits.rs
    ├── funding.rs
    └── trading.rs
```

**Commands:**
```bash
mkdir -p src/transfer/adapters
touch src/transfer/mod.rs
touch src/transfer/types.rs
touch src/transfer/state.rs
touch src/transfer/db.rs
touch src/transfer/coordinator.rs
touch src/transfer/worker.rs
touch src/transfer/adapters/mod.rs
touch src/transfer/adapters/traits.rs
touch src/transfer/adapters/funding.rs
touch src/transfer/adapters/trading.rs
```

### 1.3 Define Core Types

**File**: `src/transfer/types.rs`

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Service identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceId {
    Funding,
    Trading,
}

impl ServiceId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceId::Funding => "funding",
            ServiceId::Trading => "trading",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "funding" => Some(ServiceId::Funding),
            "trading" => Some(ServiceId::Trading),
            _ => None,
        }
    }
}

/// Unified result from any service adapter
#[derive(Debug, Clone)]
pub enum OpResult {
    /// Operation definitely succeeded
    Success,
    /// Operation definitely failed (business failure)
    Failed(String),
    /// Operation in-flight or technical error (retry)
    Pending,
}

/// Transfer record stored in database
#[derive(Debug, Clone)]
pub struct TransferRecord {
    pub req_id: Uuid,
    pub source: ServiceId,
    pub target: ServiceId,
    pub user_id: u64,
    pub asset_id: u32,
    pub amount: u64,
    pub state: TransferState,
    pub created_at: i64,
    pub updated_at: i64,
    pub error: Option<String>,
    pub retry_count: u32,
}

/// Request to create a transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    pub from: String,       // "funding" or "trading"
    pub to: String,         // "trading" or "funding"
    pub user_id: u64,
    pub asset_id: u32,
    pub amount: u64,
}

/// Response from transfer operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResponse {
    pub req_id: String,
    pub status: String,
    pub message: Option<String>,
}
```

### 1.4 Checklist

- [ ] Create `schema/transfer.cql`
- [ ] Create directory structure
- [ ] Implement `types.rs` with ServiceId, OpResult, TransferRecord
- [ ] Add `transfer` module to `src/lib.rs`
- [ ] Verify compilation: `cargo check`

---

## Phase 2: FSM & State Machine (2 hours)

### 2.1 Define State Enum

**File**: `src/transfer/state.rs`

```rust
use serde::{Deserialize, Serialize};

/// Transfer FSM states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    Init,
    SourcePending,
    SourceDone,
    TargetPending,
    Compensating,
    Committed,
    RolledBack,
    Failed,
}

impl TransferState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransferState::Init => "init",
            TransferState::SourcePending => "source_pending",
            TransferState::SourceDone => "source_done",
            TransferState::TargetPending => "target_pending",
            TransferState::Compensating => "compensating",
            TransferState::Committed => "committed",
            TransferState::RolledBack => "rolled_back",
            TransferState::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "init" => Some(TransferState::Init),
            "source_pending" => Some(TransferState::SourcePending),
            "source_done" => Some(TransferState::SourceDone),
            "target_pending" => Some(TransferState::TargetPending),
            "compensating" => Some(TransferState::Compensating),
            "committed" => Some(TransferState::Committed),
            "rolled_back" => Some(TransferState::RolledBack),
            "failed" => Some(TransferState::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self,
            TransferState::Committed |
            TransferState::RolledBack |
            TransferState::Failed
        )
    }

    /// States that need processing by scanner
    pub fn needs_processing(&self) -> bool {
        !self.is_terminal()
    }
}

/// FSM Events
#[derive(Debug, Clone)]
pub enum TransferEvent {
    SourceCall,     // Called source, got Pending
    SourceOk,       // Source returned Success
    SourceFail,     // Source returned Failed
    TargetCall,     // Called target, got Pending
    TargetOk,       // Target returned Success
    TargetFail,     // Target returned Failed
    RollbackOk,     // Rollback succeeded
    RollbackFail,   // Rollback failed (retry)
}

/// State transition function
pub fn transition(current: TransferState, event: TransferEvent) -> TransferState {
    use TransferState::*;
    use TransferEvent::*;

    match (current, event) {
        // From Init
        (Init, SourceCall) => SourcePending,
        (Init, SourceOk) => SourceDone,
        (Init, SourceFail) => Failed,

        // From SourcePending
        (SourcePending, SourceOk) => SourceDone,
        (SourcePending, SourceFail) => Failed,

        // From SourceDone
        (SourceDone, TargetCall) => TargetPending,
        (SourceDone, TargetOk) => Committed,
        (SourceDone, TargetFail) => Compensating,

        // From TargetPending
        (TargetPending, TargetOk) => Committed,
        (TargetPending, TargetFail) => Compensating,

        // From Compensating
        (Compensating, RollbackOk) => RolledBack,
        (Compensating, RollbackFail) => Compensating, // Retry

        // Invalid transitions - stay in current state
        _ => current,
    }
}
```

### 2.2 Checklist

- [ ] Implement `state.rs` with TransferState enum
- [ ] Implement TransferEvent enum
- [ ] Implement `transition()` function
- [ ] Add unit tests for all state transitions
- [ ] Verify compilation: `cargo check`

---

## Phase 3: DB Operations (2 hours)

### 3.1 Implement TransferDb

**File**: `src/transfer/db.rs`

```rust
use anyhow::Result;
use scylla::Session;
use std::sync::Arc;
use uuid::Uuid;

use crate::transfer::state::TransferState;
use crate::transfer::types::{TransferRecord, ServiceId};

pub struct TransferDb {
    session: Arc<Session>,
}

impl TransferDb {
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Create a new transfer record
    pub async fn create(&self, record: &TransferRecord) -> Result<()> {
        self.session.query(
            "INSERT INTO trading.transfers
             (req_id, source, target, user_id, asset_id, amount, state, created_at, updated_at, error, retry_count)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                record.req_id,
                record.source.as_str(),
                record.target.as_str(),
                record.user_id as i64,
                record.asset_id as i32,
                record.amount as i64,
                record.state.as_str(),
                record.created_at,
                record.updated_at,
                &record.error,
                record.retry_count as i32,
            )
        ).await?;
        Ok(())
    }

    /// Get transfer by req_id
    pub async fn get(&self, req_id: Uuid) -> Result<Option<TransferRecord>> {
        let result = self.session.query(
            "SELECT req_id, source, target, user_id, asset_id, amount, state,
                    created_at, updated_at, error, retry_count
             FROM trading.transfers WHERE req_id = ?",
            (req_id,)
        ).await?;

        if let Some(row) = result.first_row_typed::<(
            Uuid, String, String, i64, i32, i64, String, i64, i64, Option<String>, i32
        )>().ok() {
            let record = TransferRecord {
                req_id: row.0,
                source: ServiceId::from_str(&row.1).unwrap_or(ServiceId::Funding),
                target: ServiceId::from_str(&row.2).unwrap_or(ServiceId::Trading),
                user_id: row.3 as u64,
                asset_id: row.4 as u32,
                amount: row.5 as u64,
                state: TransferState::from_str(&row.6).unwrap_or(TransferState::Init),
                created_at: row.7,
                updated_at: row.8,
                error: row.9,
                retry_count: row.10 as u32,
            };
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    /// Conditional state update (returns true if applied)
    /// CRITICAL: Uses LWT for safe concurrent updates
    pub async fn update_state_if(
        &self,
        req_id: Uuid,
        expected: TransferState,
        new_state: TransferState,
    ) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();

        let result = self.session.query(
            "UPDATE trading.transfers
             SET state = ?, updated_at = ?
             WHERE req_id = ?
             IF state = ?",
            (new_state.as_str(), now, req_id, expected.as_str())
        ).await?;

        // Check [applied] column from LWT result
        // IMPORTANT: Log warning if parsing fails (expert review recommendation)
        let applied = match result.first_row_typed::<(bool,)>() {
            Ok(row) => row.0,
            Err(e) => {
                log::warn!("Failed to parse LWT result for {}: {} (treating as not applied)", req_id, e);
                false
            }
        };

        Ok(applied)
    }

    /// Update state with error message
    pub async fn update_state_with_error(
        &self,
        req_id: Uuid,
        expected: TransferState,
        new_state: TransferState,
        error: &str,
    ) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();

        let result = self.session.query(
            "UPDATE trading.transfers
             SET state = ?, updated_at = ?, error = ?
             WHERE req_id = ?
             IF state = ?",
            (new_state.as_str(), now, error, req_id, expected.as_str())
        ).await?;

        let applied = result.first_row_typed::<(bool,)>()
            .map(|r| r.0)
            .unwrap_or(false);

        Ok(applied)
    }

    /// Increment retry count and update timestamp
    pub async fn increment_retry(&self, req_id: Uuid) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();

        self.session.query(
            "UPDATE trading.transfers
             SET retry_count = retry_count + 1, updated_at = ?
             WHERE req_id = ?",
            (now, req_id)
        ).await?;

        Ok(())
    }

    /// Find stale transfers (for scanner)
    /// Returns transfers not in terminal state and updated_at < cutoff
    pub async fn find_stale(&self, stale_after_ms: i64) -> Result<Vec<TransferRecord>> {
        let cutoff = chrono::Utc::now().timestamp_millis() - stale_after_ms;

        // Note: Using ALLOW FILTERING for scan.
        // In production with high volume, consider materialized view.
        let result = self.session.query(
            "SELECT req_id, source, target, user_id, asset_id, amount, state,
                    created_at, updated_at, error, retry_count
             FROM trading.transfers
             WHERE state IN ('init', 'source_pending', 'source_done', 'target_pending', 'compensating')
               AND updated_at < ?
             ALLOW FILTERING",
            (cutoff,)
        ).await?;

        let mut records = Vec::new();
        for row in result.rows_typed::<(
            Uuid, String, String, i64, i32, i64, String, i64, i64, Option<String>, i32
        )>()? {
            let row = row?;
            records.push(TransferRecord {
                req_id: row.0,
                source: ServiceId::from_str(&row.1).unwrap_or(ServiceId::Funding),
                target: ServiceId::from_str(&row.2).unwrap_or(ServiceId::Trading),
                user_id: row.3 as u64,
                asset_id: row.4 as u32,
                amount: row.5 as u64,
                state: TransferState::from_str(&row.6).unwrap_or(TransferState::Init),
                created_at: row.7,
                updated_at: row.8,
                error: row.9,
                retry_count: row.10 as u32,
            });
        }

        Ok(records)
    }
}
```

### 3.2 Checklist

- [ ] Implement `db.rs` with TransferDb
- [ ] Implement `create()` method
- [ ] Implement `get()` method
- [ ] Implement `update_state_if()` with LWT
- [ ] Implement `update_state_with_error()`
- [ ] Implement `increment_retry()`
- [ ] Implement `find_stale()` for scanner
- [ ] Add unit tests with mock session
- [ ] Verify compilation: `cargo check`

---

## Phase 4: Service Adapter Trait & Mocks (2 hours)

### 4.1 Define Adapter Trait

**File**: `src/transfer/adapters/traits.rs`

```rust
use async_trait::async_trait;
use uuid::Uuid;

use crate::transfer::types::OpResult;

/// Service adapter trait - implemented by each service
#[async_trait]
pub trait ServiceAdapter: Send + Sync {
    /// Withdraw/debit funds from user account
    /// Idempotent: same req_id returns same result
    async fn withdraw(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult;

    /// Deposit/credit funds to user account
    /// Idempotent: same req_id returns same result
    async fn deposit(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult;

    /// Finalize a pending withdraw (for services with freeze stage)
    async fn commit(&self, req_id: Uuid) -> OpResult;

    /// Rollback a pending withdraw (compensation)
    async fn rollback(&self, req_id: Uuid) -> OpResult;

    /// Query status of an operation
    async fn query(&self, req_id: Uuid) -> OpResult;
}
```

### 4.2 Implement Mock Adapter (for testing)

**File**: `src/transfer/adapters/mock.rs`

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

use crate::transfer::types::OpResult;
use super::traits::ServiceAdapter;

/// Mock adapter for testing
pub struct MockAdapter {
    name: String,
    operations: Mutex<HashMap<Uuid, OpResult>>,
}

impl MockAdapter {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            operations: Mutex::new(HashMap::new()),
        }
    }

    /// Set expected result for a req_id
    pub fn set_result(&self, req_id: Uuid, result: OpResult) {
        self.operations.lock().unwrap().insert(req_id, result);
    }
}

#[async_trait]
impl ServiceAdapter for MockAdapter {
    async fn withdraw(&self, req_id: Uuid, _user_id: u64, _asset_id: u32, _amount: u64) -> OpResult {
        log::info!("[{}] withdraw({})", self.name, req_id);
        self.operations.lock().unwrap()
            .get(&req_id)
            .cloned()
            .unwrap_or(OpResult::Success)
    }

    async fn deposit(&self, req_id: Uuid, _user_id: u64, _asset_id: u32, _amount: u64) -> OpResult {
        log::info!("[{}] deposit({})", self.name, req_id);
        self.operations.lock().unwrap()
            .get(&req_id)
            .cloned()
            .unwrap_or(OpResult::Success)
    }

    async fn commit(&self, req_id: Uuid) -> OpResult {
        log::info!("[{}] commit({})", self.name, req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        log::info!("[{}] rollback({})", self.name, req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
        log::info!("[{}] query({})", self.name, req_id);
        self.operations.lock().unwrap()
            .get(&req_id)
            .cloned()
            .unwrap_or(OpResult::Pending)
    }
}
```

### 4.3 Implement Funding Adapter (Skeleton)

**File**: `src/transfer/adapters/funding.rs`

```rust
use async_trait::async_trait;
use uuid::Uuid;

use crate::transfer::types::OpResult;
use super::traits::ServiceAdapter;

/// Funding service adapter (has freeze/commit pattern)
pub struct FundingAdapter {
    // TODO: Add TigerBeetle client, DB, etc.
}

impl FundingAdapter {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ServiceAdapter for FundingAdapter {
    async fn withdraw(&self, req_id: Uuid, user_id: u64, asset_id: u32, amount: u64) -> OpResult {
        // TODO: Implement freeze logic
        // 1. Check idempotency (have we processed this req_id?)
        // 2. Lock funds: available -= amount, frozen += amount
        // 3. Return Pending (waiting for commit)
        log::info!("FundingAdapter::withdraw({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount);
        OpResult::Pending
    }

    async fn deposit(&self, req_id: Uuid, user_id: u64, asset_id: u32, amount: u64) -> OpResult {
        // TODO: Implement direct credit
        // 1. Check idempotency
        // 2. Credit funds: available += amount
        // 3. Return Success
        log::info!("FundingAdapter::deposit({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount);
        OpResult::Success
    }

    async fn commit(&self, req_id: Uuid) -> OpResult {
        // TODO: Finalize freeze
        // 1. Delete frozen funds
        // 2. Return Success
        log::info!("FundingAdapter::commit({})", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        // TODO: Unfreeze
        // 1. frozen -= amount, available += amount
        // 2. Return Success
        log::info!("FundingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
        // TODO: Check operation status
        log::info!("FundingAdapter::query({})", req_id);
        OpResult::Pending
    }
}
```

### 4.4 Implement Trading Adapter (Skeleton)

**File**: `src/transfer/adapters/trading.rs`

```rust
use async_trait::async_trait;
use uuid::Uuid;

use crate::transfer::types::OpResult;
use super::traits::ServiceAdapter;

/// Trading service adapter (direct operations, no freeze)
pub struct TradingAdapter {
    // TODO: Add UBSCore client, Aeron, etc.
}

impl TradingAdapter {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ServiceAdapter for TradingAdapter {
    async fn withdraw(&self, req_id: Uuid, user_id: u64, asset_id: u32, amount: u64) -> OpResult {
        // TODO: Implement direct debit via UBSCore
        // 1. Check idempotency
        // 2. Debit funds directly: available -= amount
        // 3. Return Success or Failed(insufficient)
        log::info!("TradingAdapter::withdraw({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount);
        OpResult::Success
    }

    async fn deposit(&self, req_id: Uuid, user_id: u64, asset_id: u32, amount: u64) -> OpResult {
        // TODO: Implement direct credit via UBSCore
        // 1. Check idempotency
        // 2. Credit funds: available += amount
        // 3. Return Success
        log::info!("TradingAdapter::deposit({}, user={}, asset={}, amount={})",
            req_id, user_id, asset_id, amount);
        OpResult::Success
    }

    async fn commit(&self, req_id: Uuid) -> OpResult {
        // Trading has no freeze stage, commit is no-op
        log::info!("TradingAdapter::commit({}) - no-op", req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        // TODO: Reverse with deposit if needed
        log::info!("TradingAdapter::rollback({})", req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
        // TODO: Check operation cache
        log::info!("TradingAdapter::query({})", req_id);
        OpResult::Pending
    }
}
```

### 4.5 Checklist

- [ ] Implement `traits.rs` with ServiceAdapter trait
- [ ] Implement `mock.rs` for testing
- [ ] Implement `funding.rs` skeleton
- [ ] Implement `trading.rs` skeleton
- [ ] Create `adapters/mod.rs` exports
- [ ] Add unit tests for mock adapter
- [ ] Verify compilation: `cargo check`

---

## Phase 5: Coordinator (2 hours)

### 5.1 Implement TransferCoordinator

**File**: `src/transfer/coordinator.rs`

```rust
use anyhow::Result;
use std::sync::Arc;
use uuid::Uuid;

use crate::transfer::db::TransferDb;
use crate::transfer::state::{TransferState, TransferEvent, transition};
use crate::transfer::types::{TransferRecord, TransferRequest, ServiceId, OpResult};
use crate::transfer::adapters::traits::ServiceAdapter;

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

    /// Create a new transfer
    pub async fn create(&self, req: TransferRequest) -> Result<Uuid> {
        let req_id = Uuid::new_v4();
        let now = chrono::Utc::now().timestamp_millis();

        let source = ServiceId::from_str(&req.from)
            .ok_or_else(|| anyhow::anyhow!("Invalid source: {}", req.from))?;
        let target = ServiceId::from_str(&req.to)
            .ok_or_else(|| anyhow::anyhow!("Invalid target: {}", req.to))?;

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

        Ok(req_id)
    }

    /// Get adapter for a service
    fn get_adapter(&self, service: ServiceId) -> &dyn ServiceAdapter {
        match service {
            ServiceId::Funding => self.funding_adapter.as_ref(),
            ServiceId::Trading => self.trading_adapter.as_ref(),
        }
    }

    /// Execute one step of the FSM
    /// Returns the new state after this step
    pub async fn step(&self, req_id: Uuid) -> Result<TransferState> {
        let record = self.db.get(req_id).await?
            .ok_or_else(|| anyhow::anyhow!("Transfer not found: {}", req_id))?;

        // Terminal state - nothing to do
        if record.state.is_terminal() {
            return Ok(record.state);
        }

        let source = self.get_adapter(record.source);
        let target = self.get_adapter(record.target);

        let new_state = match record.state {
            TransferState::Init => {
                self.step_init(&record, source).await?
            }
            TransferState::SourcePending => {
                self.step_source_pending(&record, source).await?
            }
            TransferState::SourceDone => {
                self.step_source_done(&record, source, target).await?
            }
            TransferState::TargetPending => {
                self.step_target_pending(&record, source, target).await?
            }
            TransferState::Compensating => {
                self.step_compensating(&record, source).await?
            }
            _ => record.state,
        };

        Ok(new_state)
    }

    /// Step from Init state
    async fn step_init(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        // 1. Persist SourcePending BEFORE calling service
        if !self.db.update_state_if(record.req_id, TransferState::Init, TransferState::SourcePending).await? {
            // Another worker already transitioned
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

    /// Step from SourcePending state
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
                self.db.increment_retry(record.req_id).await?;
                Ok(TransferState::SourcePending)
            }
        }
    }

    /// Step from SourceDone state
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
                // Commit source (finalize freeze if applicable)
                // EXPERT REVIEW: Log if commit fails, but proceed with Committed state
                // because target has already received the funds
                let commit_result = source.commit(record.req_id).await;
                if let OpResult::Failed(e) = &commit_result {
                    log::warn!("Source commit failed for {} (target already received funds): {}",
                        record.req_id, e);
                    // TODO: Send alert to ops for manual cleanup of frozen funds
                }
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

    /// Step from TargetPending state
    async fn step_target_pending(
        &self,
        record: &TransferRecord,
        source: &dyn ServiceAdapter,
        target: &dyn ServiceAdapter,
    ) -> Result<TransferState> {
        // Query or re-call target (idempotent)
        let result = target.deposit(
            record.req_id,
            record.user_id,
            record.asset_id,
            record.amount,
        ).await;

        match result {
            OpResult::Success => {
                source.commit(record.req_id).await;
                self.db.update_state_if(record.req_id, TransferState::TargetPending, TransferState::Committed).await?;
                Ok(TransferState::Committed)
            }
            OpResult::Failed(e) => {
                self.db.update_state_with_error(record.req_id, TransferState::TargetPending, TransferState::Compensating, &e).await?;
                Ok(TransferState::Compensating)
            }
            OpResult::Pending => {
                self.db.increment_retry(record.req_id).await?;
                Ok(TransferState::TargetPending)
            }
        }
    }

    /// Step from Compensating state
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
            OpResult::Failed(_) | OpResult::Pending => {
                // Keep retrying rollback
                self.db.increment_retry(record.req_id).await?;
                Ok(TransferState::Compensating)
            }
        }
    }
}
```

### 5.2 Checklist

- [ ] Implement `coordinator.rs`
- [ ] Implement `create()` method
- [ ] Implement `step()` method
- [ ] Implement step_init, step_source_pending, step_source_done, step_target_pending, step_compensating
- [ ] Add unit tests with mock adapters
- [ ] Test all state transitions
- [ ] Verify compilation: `cargo check`

---

## Phase 6: Background Worker (2 hours)

### 6.1 Implement Worker

**File**: `src/transfer/worker.rs`

```rust
use anyhow::Result;
use crossbeam::queue::ArrayQueue;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::transfer::coordinator::TransferCoordinator;
use crate::transfer::db::TransferDb;
use crate::transfer::state::TransferState;

/// Worker configuration
pub struct WorkerConfig {
    /// Scan interval for stale transfers (ms)
    pub scan_interval_ms: u64,
    /// Process timeout for sync calls (ms)
    pub process_timeout_ms: u64,
    /// Delay before retrying stale transfers (ms)
    pub stale_after_ms: i64,
    /// Max retries before alert
    pub alert_threshold: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            scan_interval_ms: 5000,     // 5 seconds
            process_timeout_ms: 2000,   // 2 seconds
            stale_after_ms: 30000,      // 30 seconds
            alert_threshold: 10,
        }
    }
}

/// Transfer queue (ring buffer)
pub struct TransferQueue {
    buffer: ArrayQueue<Uuid>,
}

impl TransferQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: ArrayQueue::new(capacity),
        }
    }

    pub fn try_push(&self, req_id: Uuid) -> bool {
        self.buffer.push(req_id).is_ok()
    }

    pub fn try_pop(&self) -> Option<Uuid> {
        self.buffer.pop()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}

/// Background worker
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

    /// Process a single transfer synchronously with timeout
    /// For Gateway happy path
    pub async fn process_now(&self, req_id: Uuid) -> TransferState {
        let deadline = Instant::now() + Duration::from_millis(self.config.process_timeout_ms);

        loop {
            match self.coordinator.step(req_id).await {
                Ok(state) => {
                    if state.is_terminal() {
                        return state;
                    }

                    if Instant::now() >= deadline {
                        return state;
                    }

                    // Small delay before next step
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

    /// Run the background worker loop
    pub async fn run(&self) {
        log::info!("Transfer worker started");

        loop {
            // 1. Process items from queue
            while let Some(req_id) = self.queue.try_pop() {
                self.process_transfer(req_id).await;
            }

            // 2. Scan for stale transfers
            match self.db.find_stale(self.config.stale_after_ms).await {
                Ok(stale) => {
                    for record in stale {
                        log::info!("Found stale transfer: {} (state: {:?}, retries: {})",
                            record.req_id, record.state, record.retry_count);

                        // Check alert threshold
                        if record.retry_count >= self.config.alert_threshold {
                            log::warn!("ALERT: Transfer {} stuck after {} retries",
                                record.req_id, record.retry_count);
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

    /// Process a single transfer to completion (or until non-terminal + backoff)
    async fn process_transfer(&self, req_id: Uuid) {
        loop {
            match self.coordinator.step(req_id).await {
                Ok(state) => {
                    if state.is_terminal() {
                        log::info!("Transfer {} completed: {:?}", req_id, state);
                        return;
                    }

                    // Not terminal, but we've done one step
                    // Let the scanner pick it up again after stale_after_ms
                    return;
                }
                Err(e) => {
                    log::error!("Error processing transfer {}: {}", req_id, e);
                    return;
                }
            }
        }
    }
}
```

### 6.2 Checklist

- [ ] Implement `worker.rs`
- [ ] Implement TransferQueue with ring buffer
- [ ] Implement WorkerConfig with defaults
- [ ] Implement `process_now()` for sync processing
- [ ] Implement `run()` for background loop
- [ ] Implement scanner logic
- [ ] Add alerting logic
- [ ] Add unit tests
- [ ] Verify compilation: `cargo check`

---

## Phase 7: Gateway Integration (2 hours)

### 7.1 Add Transfer Endpoint

**File**: `src/gateway.rs` (add new handler)

```rust
use crate::transfer::types::{TransferRequest, TransferResponse};
use crate::transfer::coordinator::TransferCoordinator;
use crate::transfer::worker::{TransferWorker, TransferQueue};
use crate::transfer::state::TransferState;

/// POST /api/v1/transfer
pub async fn handle_transfer(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferRequest>,
) -> impl IntoResponse {
    // 1. Validate request
    if req.amount == 0 {
        return Json(TransferResponse {
            req_id: String::new(),
            status: "failed".to_string(),
            message: Some("Amount must be positive".to_string()),
        });
    }

    // 2. Create transfer
    let req_id = match state.coordinator.create(req).await {
        Ok(id) => id,
        Err(e) => {
            return Json(TransferResponse {
                req_id: String::new(),
                status: "failed".to_string(),
                message: Some(format!("Failed to create transfer: {}", e)),
            });
        }
    };

    // 3. SYNC: Try full processing immediately (happy path)
    let result = state.worker.process_now(req_id).await;

    // 4. Return based on result
    let (status, message) = match result {
        TransferState::Committed => ("committed", None),
        TransferState::RolledBack => ("rolled_back", Some("Transfer cancelled")),
        TransferState::Failed => ("failed", Some("Transfer failed")),
        _ => {
            // Pending - push to queue for background processing
            state.queue.try_push(req_id);
            ("pending", Some("Processing in background"))
        }
    };

    Json(TransferResponse {
        req_id: req_id.to_string(),
        status: status.to_string(),
        message: message.map(|s| s.to_string()),
    })
}

/// GET /api/v1/transfer/{req_id}
pub async fn get_transfer(
    State(state): State<Arc<AppState>>,
    Path(req_id_str): Path<String>,
) -> impl IntoResponse {
    let req_id = match Uuid::parse_str(&req_id_str) {
        Ok(id) => id,
        Err(_) => {
            return Json(serde_json::json!({
                "error": "Invalid req_id format"
            }));
        }
    };

    match state.db.get(req_id).await {
        Ok(Some(record)) => {
            Json(serde_json::json!({
                "req_id": record.req_id.to_string(),
                "state": record.state.as_str(),
                "source": record.source.as_str(),
                "target": record.target.as_str(),
                "amount": record.amount,
                "created_at": record.created_at,
                "updated_at": record.updated_at,
                "error": record.error,
                "retry_count": record.retry_count,
            }))
        }
        Ok(None) => {
            Json(serde_json::json!({
                "error": "Transfer not found"
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "error": format!("Database error: {}", e)
            }))
        }
    }
}
```

### 7.2 Update AppState

```rust
pub struct AppState {
    // ... existing fields ...

    // Transfer
    pub coordinator: Arc<TransferCoordinator>,
    pub worker: Arc<TransferWorker>,
    pub queue: Arc<TransferQueue>,
}
```

### 7.3 Register Routes

```rust
let app = Router::new()
    // ... existing routes ...
    .route("/api/v1/transfer", post(handle_transfer))
    .route("/api/v1/transfer/:req_id", get(get_transfer))
    .with_state(state);
```

### 7.4 Checklist

- [ ] Add TransferRequest/TransferResponse types
- [ ] Implement `handle_transfer()` endpoint
- [ ] Implement `get_transfer()` endpoint
- [ ] Update AppState with coordinator, worker, queue
- [ ] Register routes
- [ ] Start worker in background on server startup
- [ ] Verify compilation: `cargo check`
- [ ] Manual test with curl

---

## Phase 8: E2E Testing (3 hours)

### 8.1 Create E2E Test Script

**File**: `tests/20_transfer_e2e.sh`

```bash
#!/bin/bash
set -e

echo "🧪 TRANSFER E2E TEST"

# Cleanup
cleanup() {
    pkill -f order_gate_server || true
}
trap cleanup EXIT

# Start gateway
RUST_LOG=info ./target/debug/order_gate_server &
GATEWAY_PID=$!
sleep 3

BASE_URL="http://127.0.0.1:8080"

echo "📤 Test 1: Funding → Trading (happy path)"
RESP=$(curl -s -X POST "$BASE_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d '{"from":"funding","to":"trading","user_id":4001,"asset_id":1,"amount":1000000}')
echo "Response: $RESP"

STATUS=$(echo "$RESP" | jq -r '.status')
if [ "$STATUS" == "committed" ]; then
    echo "✅ Test 1 PASSED"
else
    echo "❌ Test 1 FAILED: expected committed, got $STATUS"
    exit 1
fi

echo "📤 Test 2: Trading → Funding (happy path)"
RESP=$(curl -s -X POST "$BASE_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d '{"from":"trading","to":"funding","user_id":4001,"asset_id":1,"amount":500000}')
echo "Response: $RESP"

STATUS=$(echo "$RESP" | jq -r '.status')
if [ "$STATUS" == "committed" ]; then
    echo "✅ Test 2 PASSED"
else
    echo "❌ Test 2 FAILED: expected committed, got $STATUS"
    exit 1
fi

echo "📤 Test 3: Query transfer"
REQ_ID=$(echo "$RESP" | jq -r '.req_id')
RESP=$(curl -s "$BASE_URL/api/v1/transfer/$REQ_ID")
echo "Response: $RESP"

STATE=$(echo "$RESP" | jq -r '.state')
if [ "$STATE" == "committed" ]; then
    echo "✅ Test 3 PASSED"
else
    echo "❌ Test 3 FAILED: expected committed, got $STATE"
    exit 1
fi

# EXPERT REVIEW: Additional test cases

echo "📤 Test 4: Invalid amount (zero)"
RESP=$(curl -s -X POST "$BASE_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d '{"from":"funding","to":"trading","user_id":4001,"asset_id":1,"amount":0}')
echo "Response: $RESP"

STATUS=$(echo "$RESP" | jq -r '.status')
if [ "$STATUS" == "failed" ]; then
    echo "✅ Test 4 PASSED (correctly rejected zero amount)"
else
    echo "❌ Test 4 FAILED: expected failed, got $STATUS"
    exit 1
fi

echo "📤 Test 5: Invalid source"
RESP=$(curl -s -X POST "$BASE_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d '{"from":"invalid","to":"trading","user_id":4001,"asset_id":1,"amount":1000}')
echo "Response: $RESP"

STATUS=$(echo "$RESP" | jq -r '.status')
if [ "$STATUS" == "failed" ]; then
    echo "✅ Test 5 PASSED (correctly rejected invalid source)"
else
    echo "❌ Test 5 FAILED: expected failed, got $STATUS"
    exit 1
fi

echo "📤 Test 6: Query non-existent transfer"
RESP=$(curl -s "$BASE_URL/api/v1/transfer/00000000-0000-0000-0000-000000000000")
echo "Response: $RESP"

ERROR=$(echo "$RESP" | jq -r '.error')
if [ "$ERROR" == "Transfer not found" ]; then
    echo "✅ Test 6 PASSED (correctly returned not found)"
else
    echo "❌ Test 6 FAILED: expected 'Transfer not found', got $ERROR"
    exit 1
fi

echo ""
echo "🎉 ALL TESTS PASSED!"
```

### 8.2 Checklist

- [ ] Create E2E test script
- [ ] Test Funding → Trading
- [ ] Test Trading → Funding
- [ ] Test query endpoint
- [ ] Test error cases (insufficient funds, invalid amount)
- [ ] Test pending state (simulate slow service)
- [ ] Test recovery (stop/start worker)
- [ ] All tests pass: `./tests/20_transfer_e2e.sh`

---

## Summary

### Total Estimated Time: 17 hours

| Phase | Time | Status |
|-------|------|--------|
| 1. Database Schema & Types | 2h | ⬜ |
| 2. FSM & State Machine | 2h | ⬜ |
| 3. DB Operations | 2h | ⬜ |
| 4. Service Adapters | 2h | ⬜ |
| 5. Coordinator | 2h | ⬜ |
| 6. Background Worker | 2h | ⬜ |
| 7. Gateway Integration | 2h | ⬜ |
| 8. E2E Testing | 3h | ⬜ |

### Files to Create

```
schema/transfer.cql
src/transfer/mod.rs
src/transfer/types.rs
src/transfer/state.rs
src/transfer/db.rs
src/transfer/coordinator.rs
src/transfer/worker.rs
src/transfer/adapters/mod.rs
src/transfer/adapters/traits.rs
src/transfer/adapters/mock.rs
src/transfer/adapters/funding.rs
src/transfer/adapters/trading.rs
tests/20_transfer_e2e.sh
```

### Ready to Start!

Would you like me to begin with Phase 1?
