# UBSCore Implementation Plan

**Version**: 1.0
**Created**: 2025-12-08
**Status**: PLANNING

---

## Executive Summary

Implement **UBSCore** (User Balance Service Core) - the in-memory balance authority that validates orders before they reach the Matching Engine.

**Target**: ~50 µs end-to-end order entry latency

---

## Implementation Phases

```
Phase 9:  Foundation (Core Data Structures)
Phase 10: Persistence (WAL Module)
Phase 11: Communication (Aeron Integration)
Phase 12: Integration (Connect Existing Services)
Phase 13: Production Hardening
```

---

## Phase 9: Foundation (Core Data Structures)

**Goal**: Build the in-memory core without persistence or networking.

### Task 9.1: Create UBSCore Crate Structure

**Files to Create**:
```
src/ubs_core/
├── mod.rs           # Module exports
├── account.rs       # Account struct with checked arithmetic
├── balance.rs       # Balance type with fixed-point arithmetic
├── order.rs         # Order struct for internal use
├── dedup.rs         # Deduplication guard (IndexSet)
├── risk_model.rs    # RiskModel trait + SpotRiskModel
└── error.rs         # RejectReason enum
```

**Status**: 📋 NOT STARTED

---

### Task 9.2: ✅ EXISTING - Balance with Checked Arithmetic

**Already Implemented**: `src/user_account.rs`

The existing `Balance` struct already has checked arithmetic:

```rust
// EXISTING CODE (src/user_account.rs)
pub type UserId = u64;
pub type AssetId = u32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Balance {
    pub avail: u64,    // NOT "available" - use concise names
    pub frozen: u64,
    pub version: u64,
}

impl Balance {
    pub fn deposit(&mut self, amount: u64) -> Result<(), &'static str> {
        self.avail = self.avail.checked_add(amount).ok_or("Balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn frozen(&mut self, amount: u64) -> Result<(), &'static str> {
        // Checked arithmetic already!
        self.avail = self.avail.checked_sub(amount).ok_or("Balance underflow")?;
        self.frozen = self.frozen.checked_add(amount).ok_or("Frozen balance overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }

    pub fn unfrozen(&mut self, amount: u64) -> Result<(), &'static str> { ... }
    pub fn spend_frozen(&mut self, amount: u64) -> Result<(), &'static str> { ... }
}

pub struct UserAccount {
    pub user_id: UserId,
    pub assets: Vec<(AssetId, Balance)>,  // Per-asset balances
}
```

**Status**: ✅ ALREADY EXISTS

**Note**: UBSCore should **reuse** this existing code, not duplicate it.

---

### Task 9.2b: Add `speculative` field to Balance (OPTIONAL)

If hot path speculative credits are needed, extend the existing `Balance`:

```rust
// Extension to existing Balance struct
pub struct Balance {
    pub avail: u64,
    pub frozen: u64,
    pub speculative: u64,  // NEW: Hot path credits
    pub version: u64,
}
```

**Status**: 📋 NOT STARTED (optional for MVP)

---

### Task 9.3: Implement Order Cost Calculation

**File**: `src/ubs_core/order.rs`

```rust
/// Order for internal processing
/// Gateway converts decimals → raw u64 BEFORE sending to UBSCore
#[derive(Debug, Clone)]
pub struct Order {
    pub order_id: HalfUlid,
    pub user_id: UserId,
    pub symbol_id: u32,
    pub side: Side,
    pub price: u64,      // Raw u64 (Gateway already scaled)
    pub qty: u64,        // Raw u64 (Gateway already scaled)
    pub order_type: OrderType,
}

impl Order {
    /// Calculate order cost internally (SECURITY: never trust Gateway's cost field)
    ///
    /// UBSCore uses raw u64 - Gateway handles decimal scaling
    /// This is fast, simple, robust - pure state machine
    pub fn calculate_cost(&self) -> u64 {
        match self.side {
            Side::Buy => {
                // Buy: pay quote asset (price × qty)
                self.price
                    .checked_mul(self.qty)
                    .unwrap_or(u64::MAX) // Overflow = reject
            }
            Side::Sell => {
                // Sell: pay base asset (qty)
                self.qty
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderType { Limit, Market }
```

**Key Design**:
- **No decimals in UBSCore** - Gateway handles scaling
- **Raw u64 everywhere** - simple, fast, robust
- **Pure state machine** - no floating point, no conversion

**Acceptance Criteria**:
- [x] Cost = price × qty (raw u64)
- [x] Overflow → u64::MAX → reject
- [ ] Unit tests

**Status**: 📋 NOT STARTED

---

### Task 9.4: Implement Deduplication Guard

**File**: `src/ubs_core/dedup.rs`

```rust
use indexmap::IndexSet;
use crate::half_ulid::HalfUlid;

const CACHE_SIZE: usize = 10_000;
const MAX_TIME_DRIFT_MS: u64 = 3_000;

pub struct DeduplicationGuard {
    cache: IndexSet<HalfUlid>,
    min_allowed_ts: u64,
}

impl DeduplicationGuard {
    pub fn new() -> Self {
        Self {
            cache: IndexSet::with_capacity(CACHE_SIZE),
            min_allowed_ts: 0,
        }
    }

    /// Check and record order ID
    /// Returns Ok if new, Err if duplicate or too old
    pub fn check_and_record(
        &mut self,
        order_id: HalfUlid,
        now: u64
    ) -> Result<(), RejectReason> {
        let order_ts = order_id.timestamp_ms();

        // 1. TIME CHECK
        if now.saturating_sub(order_ts) > MAX_TIME_DRIFT_MS {
            return Err(RejectReason::OrderTooOld);
        }
        if order_ts > now.saturating_add(1_000) {
            return Err(RejectReason::FutureTimestamp);
        }
        if order_ts < self.min_allowed_ts {
            return Err(RejectReason::OrderTooOld);
        }

        // 2. DUPLICATE CHECK
        if self.cache.contains(&order_id) {
            return Err(RejectReason::DuplicateOrderId);
        }

        // 3. EVICT IF FULL
        if self.cache.len() >= CACHE_SIZE {
            if let Some(evicted) = self.cache.pop() {
                self.min_allowed_ts = self.min_allowed_ts.max(evicted.timestamp_ms());
            }
        }

        // 4. INSERT
        self.cache.insert(order_id);

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    OrderTooOld,
    FutureTimestamp,
    DuplicateOrderId,
    InsufficientBalance,
    AccountNotFound,
    InvalidSymbol,
    OrderCostOverflow,
    SystemBusy,  // Backpressure: pending queue exceeded
}
```

**Acceptance Criteria**:
- [ ] IndexSet for O(1) lookup + ordered eviction
- [ ] Time-based rejection
- [ ] Boundary tracking for evicted entries
- [ ] Unit tests for all scenarios

**Status**: 📋 NOT STARTED

---

### Task 9.5: Implement RiskModel Trait

**File**: `src/ubs_core/risk_model.rs`

```rust
use crate::account::Account;
use crate::order::Order;

/// Trait for different market types
pub trait RiskModel: Send + Sync {
    /// Check if account can afford the order
    fn can_trade(&self, account: &Account, order: &Order) -> bool;

    /// Get the asset ID that will be debited
    fn get_debit_asset(&self, order: &Order) -> u32;
}

/// Spot market: simple balance check
pub struct SpotRiskModel;

impl RiskModel for SpotRiskModel {
    fn can_trade(&self, account: &Account, order: &Order) -> bool {
        let cost = order.calculate_cost();
        account.available() >= cost
    }

    fn get_debit_asset(&self, order: &Order) -> u32 {
        match order.side {
            Side::Buy => order.symbol_id.quote_asset(),
            Side::Sell => order.symbol_id.base_asset(),
        }
    }
}

// Future: FuturesRiskModel with margin calculations
```

**Acceptance Criteria**:
- [ ] Trait defined for extensibility
- [ ] SpotRiskModel implemented
- [ ] Unit tests

**Status**: 📋 NOT STARTED

---

### Task 9.6: Implement UBSCore Struct

**File**: `src/ubs_core/mod.rs`

```rust
use std::collections::HashMap;
use std::collections::VecDeque;

const HIGH_WATER_MARK: usize = 10_000;  // Backpressure threshold

pub struct UBSCore<R: RiskModel> {
    // State
    accounts: HashMap<u64, Account>,
    dedup_guard: DeduplicationGuard,
    pending_queue: VecDeque<Order>,  // For backpressure check

    // Logic
    risk_model: R,

    // Mode
    is_replay_mode: bool,
}

impl<R: RiskModel> UBSCore<R> {
    pub fn new(risk_model: R) -> Self {
        Self {
            accounts: HashMap::new(),
            dedup_guard: DeduplicationGuard::new(),
            pending_queue: VecDeque::new(),
            risk_model,
            is_replay_mode: false,
        }
    }

    /// Process incoming order
    pub fn process_order(&mut self, order: Order) -> Result<(), RejectReason> {
        // 0. BACKPRESSURE CHECK (pending queue depth)
        if self.pending_queue.len() > HIGH_WATER_MARK {
            return Err(RejectReason::SystemBusy);
        }

        // 1. Deduplication check
        let now = if self.is_replay_mode {
            order.order_id.timestamp_ms()
        } else {
            current_time_ms()
        };
        self.dedup_guard.check_and_record(order.order_id, now)?;

        // 2. Get account
        let account = self.accounts
            .get_mut(&order.user_id)
            .ok_or(RejectReason::AccountNotFound)?;

        // 3. Calculate cost internally (SECURITY)
        let cost = order.calculate_cost();
        if cost == u64::MAX {
            return Err(RejectReason::OrderCostOverflow);
        }

        // 4. Risk check
        if !self.risk_model.can_trade(account, &order) {
            return Err(RejectReason::InsufficientBalance);
        }

        // 5. Lock funds
        account.lock_funds(cost)?;

        // 6. Return success (caller handles WAL + ME forwarding)
        Ok(())
    }

    /// Handle trade execution (from ME)
    pub fn on_trade(&mut self, trade: Trade) -> Result<(), BalanceError> {
        // Consume frozen from seller
        let seller = self.accounts.get_mut(&trade.seller_id)
            .ok_or(BalanceError::AccountNotFound)?;
        seller.consume_frozen(trade.base_qty)?;

        // Speculative credit to buyer (hot path)
        let buyer = self.accounts.get_mut(&trade.buyer_id)
            .ok_or(BalanceError::AccountNotFound)?;
        buyer.apply_speculative_credit(trade.base_qty)?;

        Ok(())
    }

    /// Deposit funds (from blockchain)
    pub fn on_deposit(&mut self, user_id: u64, amount: u64) -> Result<(), BalanceError> {
        let account = self.accounts
            .entry(user_id)
            .or_insert_with(|| Account::new(user_id, 0));

        account.available = account.available
            .checked_add(amount)
            .ok_or(BalanceError::Overflow)?;

        Ok(())
    }
}
```

**Acceptance Criteria**:
- [ ] Order processing with all checks
- [ ] Trade handling (hot path speculative credit)
- [ ] Deposit handling
- [ ] Unit tests for main flows

**Status**: 📋 NOT STARTED

---

## Phase 10: Persistence (WAL Module)

**Goal**: Add crash-safe persistence.

### Task 10.1: Implement AlignedBuffer (O_DIRECT support)

**File**: `src/ubs_core/wal/aligned_buffer.rs`

```rust
/// Aligned buffer for O_DIRECT writes
pub struct AlignedBuffer {
    data: Vec<u8>,
    alignment: usize,
}
```

**Status**: 📋 NOT STARTED

---

### Task 10.2: Implement WAL Entry Format

**File**: `src/ubs_core/wal/entry.rs`

```
Format: [Length: u32][CRC32: u32][Payload: bytes]
```

**Status**: 📋 NOT STARTED

---

### Task 10.3: Implement Group Commit WAL

**File**: `src/ubs_core/wal/group_commit.rs`

```rust
/// WAL with group commit (batch fsync)
pub struct GroupCommitWal {
    file: File,
    buffer: AlignedBuffer,
    pending_count: usize,
    max_batch_size: usize,
}
```

**Status**: 📋 NOT STARTED

---

### Task 10.4: Implement WAL Replay

**File**: `src/ubs_core/wal/replay.rs`

**Status**: 📋 NOT STARTED

---

## Phase 11: Communication (Aeron Integration)

**Goal**: Connect UBSCore to Gateway and Matching Engine.

### Task 11.1: Aeron Subscriber (from Gateway)

**Status**: 📋 NOT STARTED

---

### Task 11.2: Aeron Publisher (to Matching Engine)

**Status**: 📋 NOT STARTED

---

### Task 11.3: Trade Consumer (from Matching Engine)

**Status**: 📋 NOT STARTED

---

## Phase 12: Integration

**Goal**: Connect UBSCore to existing services.

### Task 12.1: Replace Gateway → ME direct path

**Status**: 📋 NOT STARTED

---

### Task 12.2: Connect Settlement Service to UBSCore

**Status**: 📋 NOT STARTED

---

## Phase 13: Production Hardening

**Goal**: Metrics, monitoring, and optimizations.

### Task 13.1: Add Metrics

**Status**: 📋 NOT STARTED

---

### Task 13.2: Add Health Checks

**Status**: 📋 NOT STARTED

---

### Task 13.3: Performance Testing

**Status**: 📋 NOT STARTED

---

## Task Dependency Graph

```
Phase 9 (Foundation)
├── 9.1 Crate Structure
├── 9.2 Account (checked arithmetic) ← PRIORITY
├── 9.3 Order Cost Calculation ← PRIORITY
├── 9.4 Deduplication Guard
├── 9.5 RiskModel Trait
└── 9.6 UBSCore Struct (depends on 9.2-9.5)

Phase 10 (Persistence)
├── 10.1 AlignedBuffer
├── 10.2 WAL Entry Format
├── 10.3 Group Commit WAL (depends on 10.1, 10.2)
└── 10.4 WAL Replay (depends on 10.3)

Phase 11 (Communication)
├── 11.1 Aeron Subscriber
├── 11.2 Aeron Publisher
└── 11.3 Trade Consumer

Phase 12 (Integration)
├── 12.1 Gateway Integration (depends on 11.1)
└── 12.2 Settlement Integration

Phase 13 (Hardening)
├── 13.1 Metrics
├── 13.2 Health Checks
└── 13.3 Performance Testing
```

---

## Estimated Timeline

| Phase | Tasks | Duration | Dependencies |
|-------|-------|----------|--------------|
| **Phase 9** | 6 tasks | 1-2 weeks | None |
| **Phase 10** | 4 tasks | 1 week | Phase 9 |
| **Phase 11** | 3 tasks | 1 week | Phase 9 |
| **Phase 12** | 2 tasks | 1 week | Phase 10, 11 |
| **Phase 13** | 3 tasks | 1 week | Phase 12 |

**Total Estimate**: 5-7 weeks

---

## Priority Order (What to Build First)

### 🔴 Critical Path (Must Have)

1. **Task 9.2**: Account with checked arithmetic
2. **Task 9.3**: Order cost calculation
3. **Task 9.4**: Deduplication guard
4. **Task 9.6**: UBSCore struct
5. **Task 10.3**: Group Commit WAL

### 🟡 High Priority (Should Have)

6. **Task 10.4**: WAL Replay
7. **Task 11.1-11.3**: Aeron communication

### 🟢 Future (Nice to Have)

8. Phase 12-13: Integration and hardening

---

## Success Criteria

| Metric | Target |
|--------|--------|
| Order validation latency | < 1 µs |
| WAL write latency (batched) | < 20 µs |
| End-to-end order entry | < 50 µs |
| Deduplication accuracy | 100% |
| Crash recovery | Complete |

---

## Related Documentation

| Document | Purpose |
|----------|---------|
| [UBSCORE_ARCHITECTURE.md](./UBSCORE_ARCHITECTURE.md) | Full architecture |
| [UBSCORE_REVIEW.md](./UBSCORE_REVIEW.md) | Design review |
| [WAL_SAFETY.md](./WAL_SAFETY.md) | WAL persistence |
| [SPECULATIVE_EXECUTION.md](./SPECULATIVE_EXECUTION.md) | Hot/Cold path |
