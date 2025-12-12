# Internal Transfer - Architecture Design

**Version**: 2.0
**Date**: 2025-12-12
**Status**: ✅ Expert Reviewed - Ready for Implementation

---

## 1. Overview

A simplified, robust internal transfer system using:
- **Finite State Machine (FSM)** for deterministic state management
- **Service Adapters** to abstract different backend APIs
- **Background Worker** to drive transfers to completion
- **UUID-based idempotency** for safe retry

---

## 2. Motivation

### Problems with v1
1. **Complexity**: Multiple services (Gateway, UBSCore, Settlement, TigerBeetle) with interleaved logic
2. **Blocking**: TigerBeetle operations blocked tokio runtime
3. **Tight Coupling**: State machine spread across services
4. **Hard to Test**: Required full infrastructure

### Goals for v2
1. **Simple**: Single FSM drives all transfers
2. **Robust**: Every state can be recovered via query/retry
3. **Extensible**: Easy to add new services (adapters)
4. **Testable**: Mock adapters for unit testing

---

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                 GATEWAY                                      │
│  1. Validate request                                                         │
│  2. Create TransferRecord (state=Init)                                       │
│  3. Trigger worker                                                           │
│  4. Poll for quick result (happy path)                                       │
│  5. Return: Success / Pending / Failed                                       │
└─────────────────────────────────────────┬───────────────────────────────────┘
                                          │ trigger
                                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           TRANSFER COORDINATOR                               │
│                                                                              │
│  TransferRecord {                                                            │
│      req_id: Uuid,                                                           │
│      source: ServiceId,     // Funding or Trading                            │
│      target: ServiceId,     // Trading or Funding                            │
│      state: TransferState,  // FSM state                                     │
│  }                                                                           │
│                                                                              │
│  step(req_id) → advances FSM by one step                                     │
└─────────────────────────────────────────┬───────────────────────────────────┘
                                          │ uses
                                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            SERVICE ADAPTERS                                  │
│                                                                              │
│  ┌─────────────────────────┐         ┌─────────────────────────┐            │
│  │   FundingAdapter (A)    │         │   TradingAdapter (B)    │            │
│  │                         │         │                         │            │
│  │  Internal: freeze/      │         │  Internal: direct       │            │
│  │  commit/rollback        │         │  withdraw/deposit       │            │
│  │                         │         │                         │            │
│  │  External: OpResult     │         │  External: OpResult     │            │
│  │  (Success/Pending/Fail) │         │  (Success/Pending/Fail) │            │
│  └─────────────────────────┘         └─────────────────────────┘            │
│                                                                              │
│  All adapters implement: ServiceAdapter trait                                │
│  All return: OpResult { Success, Pending, Failed }                           │
│  All support: query(req_id) for idempotent retry                             │
└─────────────────────────────────────────────────────────────────────────────┘
                                          │
                                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                          BACKGROUND WORKER                                   │
│                                                                              │
│  1. Pop from queue or scan DB for non-terminal transfers                     │
│  2. Call coordinator.step(req_id)                                            │
│  3. Repeat until terminal state (Committed/RolledBack/Failed)                │
│  4. Handle timeouts, retries, alerts                                         │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. State Machine

### 4.1 States

| State | Description | Terminal? |
|-------|-------------|-----------|
| `Init` | Transfer created, waiting for processing | No |
| `SourcePending` | Called source service, waiting for result | No |
| `SourceDone` | Source confirmed debit/freeze | No |
| `TargetPending` | Called target service, waiting for result | No |
| `Compensating` | Target failed, rolling back source | No |
| `Committed` | Transfer complete ✅ | Yes |
| `RolledBack` | Transfer cancelled, source restored ❌ | Yes |
| `Failed` | Source failed, no changes made ❌ | Yes |

### 4.2 Events (Inputs)

| Event | Trigger |
|-------|---------|
| `SourceCall` | Called source.withdraw(), got Pending |
| `SourceOk` | Source returned Success |
| `SourceFail` | Source returned Failed |
| `TargetCall` | Called target.deposit(), got Pending |
| `TargetOk` | Target returned Success |
| `TargetFail` | Target returned Failed |
| `RollbackOk` | Source rollback Success |
| `RollbackFail` | Source rollback Failed (retry) |
| `Retry` | Query returned Pending (wait and retry) |

### 4.3 State Transition Table

```
┌─────────────────┬──────────────┬─────────────────┐
│ Current State   │ Event        │ Next State      │
├─────────────────┼──────────────┼─────────────────┤
│ Init            │ SourceCall   │ SourcePending   │
│ Init            │ SourceOk     │ SourceDone      │
│ Init            │ SourceFail   │ Failed          │
├─────────────────┼──────────────┼─────────────────┤
│ SourcePending   │ SourceOk     │ SourceDone      │
│ SourcePending   │ SourceFail   │ Failed          │
│ SourcePending   │ Retry        │ SourcePending   │
├─────────────────┼──────────────┼─────────────────┤
│ SourceDone      │ TargetCall   │ TargetPending   │
│ SourceDone      │ TargetOk     │ Committed       │
│ SourceDone      │ TargetFail   │ Compensating    │
├─────────────────┼──────────────┼─────────────────┤
│ TargetPending   │ TargetOk     │ Committed       │
│ TargetPending   │ TargetFail   │ Compensating    │
│ TargetPending   │ Retry        │ TargetPending   │
├─────────────────┼──────────────┼─────────────────┤
│ Compensating    │ RollbackOk   │ RolledBack      │
│ Compensating    │ RollbackFail │ Compensating    │
├─────────────────┼──────────────┼─────────────────┤
│ Committed       │ *            │ (terminal)      │
│ RolledBack      │ *            │ (terminal)      │
│ Failed          │ *            │ (terminal)      │
└─────────────────┴──────────────┴─────────────────┘
```

### 4.4 State Diagram

```
                              SourceFail
    ┌──────────────────────────────────────────────────────┐
    │                                                      │
    │  ┌──────┐  SourceCall   ┌───────────────┐            ▼
    │  │ Init │──────────────►│ SourcePending │        ┌────────┐
    │  └──┬───┘               └───────┬───────┘        │ Failed │
    │     │                           │                └────────┘
    │     │ SourceOk                  │ SourceOk
    │     │                           │
    │     ▼                           ▼
    │  ┌─────────────────────────────────┐
    └──│          SourceDone             │
       └───────────────┬─────────────────┘
                       │
          ┌────────────┼────────────┐
          │ TargetCall │ TargetOk   │ TargetFail
          ▼            │            ▼
   ┌──────────────┐    │    ┌──────────────┐
   │TargetPending │    │    │ Compensating │
   └──────┬───────┘    │    └──────┬───────┘
          │            │           │
   TargetOk            │    RollbackOk
          │            │           │
          ▼            ▼           ▼
   ┌─────────────────────┐  ┌────────────┐
   │     Committed ✅     │  │ RolledBack │
   └─────────────────────┘  └────────────┘
```

---

## 5. Data Model

### 5.1 TransferRecord

```rust
/// Stored in database
pub struct TransferRecord {
    /// Unique identifier (UUID v4)
    pub req_id: Uuid,

    /// Source service (where funds come from)
    pub source: ServiceId,

    /// Target service (where funds go to)
    pub target: ServiceId,

    /// User performing the transfer
    pub user_id: u64,

    /// Asset being transferred
    pub asset_id: u32,

    /// Amount in smallest unit (e.g., satoshi)
    pub amount: u64,

    /// Current FSM state
    pub state: TransferState,

    /// Creation timestamp (ms)
    pub created_at: i64,

    /// Last update timestamp (ms)
    pub updated_at: i64,

    /// Error message if failed
    pub error: Option<String>,

    /// Retry count
    pub retry_count: u32,
}
```

### 5.2 ServiceId

```rust
/// Identifies a service/bank
pub enum ServiceId {
    Funding,   // Service A - has freeze/commit
    Trading,   // Service B - direct operations
}
```

### 5.3 OpResult

```rust
/// Unified result from any service adapter
pub enum OpResult {
    /// Operation definitely succeeded
    Success,

    /// Operation definitely failed (BUSINESS failure only)
    /// Examples: "Insufficient funds", "Account frozen", "Invalid user"
    Failed(String),

    /// Operation in-flight, or technical error (RETRY)
    /// Examples: Timeout, HTTP 500, connection refused
    Pending,
}
```

### 5.4 ServiceResponse Mapping (Critical!)

**Only business failures become `Failed`. Technical errors become `Pending`.**

```rust
/// Raw response from service
enum ServiceResponse {
    Success,
    BusinessFailed(String),  // Definitive rejection
    TechnicalError(String),  // Unknown state - MUST retry
    StillProcessing,         // Not done yet
}

/// Convert to OpResult
fn to_op_result(response: ServiceResponse) -> OpResult {
    match response {
        ServiceResponse::Success => OpResult::Success,

        // ONLY business failure triggers compensation
        ServiceResponse::BusinessFailed(e) => OpResult::Failed(e),

        // Technical error = unknown state = RETRY (never assume failed)
        ServiceResponse::TechnicalError(_) => OpResult::Pending,

        ServiceResponse::StillProcessing => OpResult::Pending,
    }
}
```

**Examples:**

| HTTP Status | Body | ServiceResponse | OpResult |
|-------------|------|-----------------|----------|
| 200 OK | `{"success": true}` | Success | Success |
| 200 OK | `{"error": "Insufficient funds"}` | BusinessFailed | **Failed** |
| 400 Bad Request | `{"error": "Invalid amount"}` | BusinessFailed | **Failed** |
| 500 Internal Error | - | TechnicalError | **Pending** (retry!) |
| 503 Unavailable | - | TechnicalError | **Pending** (retry!) |
| Timeout | - | TechnicalError | **Pending** (retry!) |
| Connection Refused | - | TechnicalError | **Pending** (retry!) |

---

## 6. Service Adapter Interface

### 6.1 Trait Definition

```rust
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

    /// Finalize a pending withdraw (optional)
    /// For services with freeze stage
    async fn commit(&self, req_id: Uuid) -> OpResult;

    /// Rollback a pending withdraw (compensation)
    async fn rollback(&self, req_id: Uuid) -> OpResult;

    /// Query status of an operation
    async fn query(&self, req_id: Uuid) -> OpResult;
}
```

### 6.2 Funding Adapter (Service A)

| Method | Internal Action | Returns |
|--------|-----------------|---------|
| `withdraw` | `available -= amount`, `frozen += amount` | `Pending` |
| `commit` | `frozen -= amount` | `Success` |
| `rollback` | `frozen -= amount`, `available += amount` | `Success` |
| `deposit` | `available += amount` | `Success` |
| `query` | Check frozen table by req_id | `Success/Pending/Failed` |

### 6.3 Trading Adapter (Service B)

| Method | Internal Action | Returns |
|--------|-----------------|---------|
| `withdraw` | `available -= amount` (direct) | `Success/Failed` |
| `commit` | No-op (already complete) | `Success` |
| `rollback` | Reverse with deposit | `Success` |
| `deposit` | `available += amount` | `Success` |
| `query` | Check operation cache by req_id | `Success/Failed` |

---

## 7. Component Details

### 7.1 Gateway Handler

```
1. Validate request (user, asset, amount)
2. Create TransferRecord in DB (state=Init)
3. Push req_id to worker queue (non-blocking)
4. Poll DB for 3-5 iterations (100ms each)
   - If Committed → return success
   - If RolledBack/Failed → return error
   - If still pending → continue
5. Return "pending" with req_id for client to poll
```

### 7.2 Transfer Coordinator

```
step(req_id):
1. Load TransferRecord from DB
2. Get source and target adapters
3. Based on current state:
   - Init: call source.withdraw()
   - SourcePending: call source.query()
   - SourceDone: call target.deposit()
   - TargetPending: call target.query()
   - Compensating: call source.rollback()
4. Map OpResult to FSM event
5. Transition FSM
6. Save new state to DB
7. Return new state
```

### 7.3 Background Worker

```
loop:
1. Pop req_id from queue (if any)
2. Scan DB for non-terminal transfers
3. For each transfer:
   - Call coordinator.step(req_id)
   - If not terminal, re-queue with backoff
   - If timeout exceeded, alert
4. Sleep 5 seconds
```

### 7.4 Happy Path Optimization (KISS)

**Principle**: Gateway sync-processes happy path, async handles edge cases.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          GATEWAY REQUEST FLOW                                │
│                                                                              │
│  1. Validate & Create TransferRecord (state=Init)                            │
│                                                                              │
│  2. SYNC call worker.process_now(req_id)                                     │
│     ┌────────────────────────────────────────┐                               │
│     │ Worker attempts FULL processing:       │                               │
│     │  - step(): Init → SourceDone           │                               │
│     │  - step(): SourceDone → Committed      │                               │
│     │                                        │                               │
│     │ Returns: Committed / Pending / Failed  │                               │
│     └────────────────────────────────────────┘                               │
│                                                                              │
│  3. IF result == Committed → return SUCCESS immediately ✅                   │
│     IF result == Failed → return FAILED immediately ❌                       │
│     IF result == Pending → return PENDING (worker continues in background)   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Code Example:**

```rust
async fn handle_transfer(req: TransferRequest) -> ApiResponse {
    // 1. Create record
    let req_id = coordinator.create(req).await;

    // 2. SYNC: Try full processing immediately (happy path)
    let result = worker.process_now(req_id, timeout_ms: 500).await;

    // 3. Return based on result
    match result {
        Committed => success("Transfer completed", req_id),
        Failed(e) => error(e),
        Pending => {
            // Async: Let background worker continue
            worker_queue.push(req_id);
            success_pending(req_id, "Processing in background")
        }
    }
}
```

**Worker process_now:**

```rust
impl Worker {
    /// Sync processing with timeout - for Gateway happy path
    async fn process_now(&self, req_id: Uuid, timeout_ms: u64) -> TransferState {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            let state = self.coordinator.step(req_id).await;

            // Terminal? Done!
            if state.is_terminal() {
                return state;
            }

            // Timeout? Return current state (Pending)
            if Instant::now() >= deadline {
                return state;
            }

            // Small delay before next step
            sleep(Duration::from_millis(10)).await;
        }
    }
}
```

**Benefits:**

| Scenario | Behavior |
|----------|----------|
| **Happy path** (95%+) | Gateway returns `Committed` in <500ms |
| **Slow service** | Returns `Pending`, worker continues async |
| **Service down** | Returns `Pending`, worker retries later |
| **Network timeout** | Returns `Pending`, worker recovers |

**KISS Principles:**

1. **One code path** - same `step()` logic for sync and async
2. **No polling loops** in Gateway - either done or pending
3. **Worker handles ALL edge cases** - Gateway stays simple
4. **Client gets immediate feedback** for happy path

---

## 8. Recovery Scenarios

### 8.1 Worker Crash After Source Withdraw

| State at Crash | Recovery |
|----------------|----------|
| `SourcePending` | Worker restarts, queries source, continues |

### 8.2 Source Returns Pending, Then Crashes

| State at Crash | Recovery |
|----------------|----------|
| `SourcePending` | Worker scans DB, finds pending, queries source |

### 8.3 Target Deposit Fails

| State | Action |
|-------|--------|
| `TargetFailed` → `Compensating` | Worker calls source.rollback() |
| `Compensating` → `RolledBack` | User's source balance restored |

### 8.4 Network Partition

| Scenario | Behavior |
|----------|----------|
| Can't reach source | Stay in current state, retry |
| Can't reach target | Stay in SourceDone, retry |
| Query returns Pending | Exponential backoff |

---

## 8.5 Critical Design Principles

### Principle 1: No Locking, Use Ring Buffer + Idempotent FSM

**Decision**: No distributed locks. Use ring buffer for request queueing.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         RING BUFFER PATTERN                                  │
│                                                                              │
│  Gateway                    Ring Buffer                     Worker          │
│     │                          │                               │            │
│     │─── push(req_id) ────────►│                               │            │
│     │                          │◄───────── pop() ──────────────│            │
│     │                          │                               │            │
│  Scanner                       │                               │            │
│     │─── push(req_id) ────────►│  (normally empty,             │            │
│     │                          │   immediate processing)       │            │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Why safe without locks?**

1. **FSM is deterministic**: Same state + same event = same transition
2. **DB update is idempotent**: `UPDATE ... SET state = 'X' WHERE req_id = ? AND state = 'Y'`
3. **Services are idempotent**: Same UUID → same result
4. **Race condition = harmless retry**: Both workers call `step()`, one succeeds, one sees "already transitioned"

```rust
// Safe concurrent step():
async fn step(&self, req_id: Uuid) -> TransferState {
    let record = db.get(req_id).await;

    // Compute next state
    let (event, new_state) = match record.state {
        Init => call_source_withdraw()...
        // ...
    };

    // Conditional update (only if state unchanged)
    let updated = db.update_if_state(
        req_id,
        expected_state: record.state,  // WHERE state = X
        new_state: new_state,          // SET state = Y
    ).await;

    if !updated {
        // Another worker already transitioned - that's fine!
        return db.get(req_id).await.state;
    }

    new_state
}
```

### Principle 2: DB is Source of Truth, No Complex Queue

**Decision**: No Kafka/Redis queue for transfers. DB + scanner is enough.

| Component | Role |
|-----------|------|
| **DB** | Persistent source of truth |
| **Ring Buffer** | In-memory, ephemeral, for fast path |
| **Scanner** | Finds orphaned transfers, re-queues |

**Why?**

1. **KISS**: Less infrastructure, less failure modes
2. **Happy path works**: 95%+ complete immediately
3. **Scanner handles rest**: Crash recovery, network issues
4. **No message loss**: Transfer exists in DB, scanner will find it

### Principle 3: Services Own Their Idempotency

**Decision**: Each service is responsible for idempotent operations using UUID.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                      SERVICE IDEMPOTENCY CONTRACT                            │
│                                                                              │
│  Caller provides: req_id (UUID)                                              │
│                                                                              │
│  Service guarantees:                                                         │
│    1. First call with req_id → executes operation, returns result            │
│    2. Subsequent calls with same req_id → returns SAME result (no re-execute)│
│    3. Service persists operation before returning                            │
│                                                                              │
│  Coordinator responsibility:                                                 │
│    - Generate unique req_id per transfer                                     │
│    - Pass same req_id for retries                                            │
│                                                                              │
│  Service responsibility:                                                     │
│    - Track operations by req_id                                              │
│    - Prevent double-spend/double-credit internally                           │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Principle 4: Never Auto-Refund

**Decision**: Never automatically rollback a transfer based on timeout.

**Why?**

1. **Unknown state is dangerous**: If we can't reach Service B, we don't know if it succeeded
2. **Auto-refund risk**: B succeeded, we refund A → money duplicated!
3. **Wait forever**: Keep retrying query until B responds Success or Failed
4. **Human escalation**: After N retries, alert ops team

**Rules:**

| Scenario | Action |
|----------|--------|
| Service unreachable | Retry indefinitely with backoff |
| Service returns Pending | Retry query |
| Service returns Success | Proceed |
| Service returns Failed | Compensate |
| Timeout (no response) | **NEVER** assume failed |

### Principle 5: Clear Service Responsibility Boundaries

**Decision**: Each service is 100% responsible for its own state.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                    SERVICE RESPONSIBILITY MATRIX                             │
│                                                                              │
│  Coordinator (Us):                                                           │
│    ✓ Drive FSM transitions                                                   │
│    ✓ Persist transfer state                                                  │
│    ✓ Retry on network failure                                                │
│    ✗ NOT responsible for service internal consistency                        │
│    ✗ NOT responsible for service availability                                │
│                                                                              │
│  Service A / B (Them):                                                       │
│    ✓ Execute operations atomically                                           │
│    ✓ Ensure idempotency by req_id                                            │
│    ✓ Return definitive Success/Failed                                        │
│    ✓ Handle their own crash recovery                                         │
│    ✓ Provide query endpoint for status                                       │
│                                                                              │
│  If Service B is down:                                                       │
│    → Our job: Keep retrying, alert monitoring                                │
│    → Their job: Fix their service                                            │
│    → Customer Service: Notify affected users                                 │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Principle 6: Fail-Safe Over Fail-Fast

**Decision**: When in doubt, wait. Never lose money.

| Situation | Wrong Action | Correct Action |
|-----------|--------------|----------------|
| Query timeout | Mark as Failed | Retry query |
| Service error 500 | Mark as Failed | Retry operation |
| Unknown response | Mark as Failed | Log, alert, retry |
| DB write fails | Continue | Stop, retry DB write |

**Invariant**: `Total funds = constant` must always hold.

---

## 9. Implementation Notes (Critical Patterns)

### 9.1 Conditional DB Update (MUST USE!)

```rust
/// CRITICAL: Only update if state matches expected
/// This prevents race conditions between workers

async fn update_state_if(
    &self,
    req_id: Uuid,
    expected: TransferState,
    new_state: TransferState,
) -> bool {
    // SQL: UPDATE ... WHERE req_id = ? AND state = ?
    let result = self.session.query(
        "UPDATE transfers SET state = ?, updated_at = ?
         WHERE req_id = ? IF state = ?",
        (new_state.as_str(), now_ms(), req_id, expected.as_str())
    ).await;

    result.was_applied()  // Returns false if state already changed
}
```

### 9.2 Persist-Before-Call Pattern

```rust
/// CRITICAL: Always persist state BEFORE calling external service

async fn step(&self, req_id: Uuid) -> TransferState {
    let record = db.get(req_id).await?;

    match record.state {
        Init => {
            // 1. PERSIST new state first
            if !db.update_state_if(req_id, Init, SourcePending).await {
                return db.get(req_id).await.state; // Another worker handled it
            }

            // 2. THEN call external service
            let result = source.withdraw(...).await;

            // 3. Update based on result
            match result {
                Success => db.update_state_if(req_id, SourcePending, SourceDone).await,
                Failed(e) => db.update_state_if(req_id, SourcePending, Failed).await,
                Pending => {} // Stay in SourcePending, will retry
            }
        }
        // ...
    }
}
```

### 9.3 Ring Buffer with Backpressure

```rust
use crossbeam::queue::ArrayQueue;

const BUFFER_SIZE: usize = 10_000;

struct TransferQueue {
    buffer: ArrayQueue<Uuid>,
}

impl TransferQueue {
    /// Try to push, returns false if buffer full
    fn try_push(&self, req_id: Uuid) -> bool {
        self.buffer.push(req_id).is_ok()
    }

    /// Pop next item (non-blocking)
    fn try_pop(&self) -> Option<Uuid> {
        self.buffer.pop()
    }
}

// In Gateway:
async fn handle_transfer(...) {
    let req_id = coordinator.create(req).await;

    if !queue.try_push(req_id) {
        // Buffer full - scanner will pick it up
        return success_pending(req_id, "Queued for processing");
    }

    // Try sync processing...
}
```

### 9.4 Error Handling in step()

```rust
/// Wrap service calls to handle exceptions

async fn call_service_safely<F, Fut>(f: F) -> OpResult
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<OpResult, anyhow::Error>>,
{
    match f().await {
        Ok(result) => result,
        Err(e) => {
            log::error!("Service call exception: {}", e);
            OpResult::Pending  // Treat exception as unknown state
        }
    }
}

// Usage:
let result = call_service_safely(|| source.withdraw(req_id, ...)).await;
```

### 9.5 Alerting Thresholds

```rust
const ALERT_AFTER_RETRIES: u32 = 10;
const ALERT_AFTER_SECONDS: u64 = 300;  // 5 minutes

async fn check_alerts(&self, record: &TransferRecord) {
    let age_seconds = (now_ms() - record.created_at) / 1000;

    if record.retry_count >= ALERT_AFTER_RETRIES
        || age_seconds >= ALERT_AFTER_SECONDS
    {
        alert::send(
            "Transfer stuck",
            format!("req_id={} state={} retries={} age={}s",
                record.req_id, record.state, record.retry_count, age_seconds)
        );
    }
}
```

### 9.6 Scanner Configuration

```rust
/// Make scanner interval configurable

struct WorkerConfig {
    /// Scan interval for orphaned transfers (default: 5s, prod: 1s)
    scan_interval_ms: u64,

    /// Backoff between retries
    retry_backoff_ms: u64,

    /// Max retries before alert
    alert_threshold: u32,
}
```

---

## 10. File Structure

```
src/
├── transfer/
│   ├── mod.rs              # Module exports
│   ├── fsm.rs              # State machine (rust-fsm)
│   ├── types.rs            # TransferRecord, OpResult, ServiceId
│   ├── coordinator.rs      # TransferCoordinator
│   ├── db.rs               # Database operations
│   ├── worker.rs           # Background worker
│   └── adapters/
│       ├── mod.rs          # Adapter exports
│       ├── traits.rs       # ServiceAdapter trait
│       ├── funding.rs      # FundingAdapter impl
│       └── trading.rs      # TradingAdapter impl
```

---

## 11. Database Schema

```sql
CREATE TABLE trading.transfers (
    req_id uuid PRIMARY KEY,
    source text,          -- 'funding' or 'trading'
    target text,
    user_id bigint,
    asset_id int,
    amount bigint,
    state text,           -- FSM state
    created_at bigint,
    updated_at bigint,
    error text,
    retry_count int
);

-- Index for worker scan
CREATE INDEX ON trading.transfers (state)
WHERE state NOT IN ('committed', 'rolled_back', 'failed');
```

---

## 12. API Endpoints

### Create Transfer

```
POST /api/v1/transfer
{
    "from": "funding",     // or "trading"
    "to": "trading",       // or "funding"
    "user_id": 4001,
    "asset": "BTC",
    "amount": "1.5"
}

Response:
{
    "req_id": "550e8400-e29b-41d4-a716-446655440000",
    "status": "committed" | "pending" | "failed",
    "message": "..."
}
```

### Query Transfer

```
GET /api/v1/transfer/{req_id}

Response:
{
    "req_id": "...",
    "state": "committed",
    "created_at": 1702400000000
}
```

---

## 13. Implementation Plan

| Phase | Tasks | Time |
|-------|-------|------|
| 1 | Create module structure, FSM, types | 2h |
| 2 | Implement adapters (Funding, Trading) | 3h |
| 3 | Implement coordinator | 2h |
| 4 | Implement worker | 2h |
| 5 | Update Gateway | 1h |
| 6 | Write tests | 3h |
| 7 | Migration from v1 | 2h |

**Total**: ~15 hours

---

## 14. Testing Strategy

### Unit Tests
- FSM transitions (all paths)
- Coordinator with mock adapters
- Worker with mock coordinator

### Integration Tests
- Full flow with real DB, mock services
- Recovery scenarios

### E2E Tests
- Funding → Trading
- Trading → Funding
- Failure/compensation path

---

## 15. Migration Path

1. Deploy v2 alongside v1
2. New transfers use v2
3. v1 continues for in-flight transfers
4. Deprecate v1 after drain period

---

## 16. References

- [rust-fsm crate](https://crates.io/crates/rust-fsm)
- [Saga Pattern](https://microservices.io/patterns/data/saga.html)
- [Idempotency Keys](https://stripe.com/docs/api/idempotent_requests)
