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

**Principle**: Each step is small, testable, committable.

---

### Step 9.1.1: Create Module Structure (No Logic)

**Action**: Create empty files only

```
src/ubs_core/
├── mod.rs           # pub mod declarations
├── types.rs         # Re-export existing types
├── error.rs         # RejectReason enum
├── order.rs         # InternalOrder struct
├── dedup.rs         # DeduplicationGuard
├── debt.rs          # DebtLedger
├── fee.rs           # VipFeeTable
├── risk.rs          # RiskModel trait
└── core.rs          # UBSCore struct
```

**Test**: `cargo check` passes

**Commit**: `feat(ubs_core): create module structure`

---

### Step 9.1.2: Define RejectReason Enum

**File**: `src/ubs_core/error.rs`

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    OrderTooOld,
    FutureTimestamp,
    DuplicateOrderId,
    InsufficientBalance,
    AccountNotFound,
    InvalidSymbol,
    OrderCostOverflow,
    SystemBusy,
}
```

**Unit Tests**:
```rust
#[test]
fn test_reject_reason_debug() {
    let reason = RejectReason::InsufficientBalance;
    assert_eq!(format!("{:?}", reason), "InsufficientBalance");
}

#[test]
fn test_reject_reason_clone() {
    let reason = RejectReason::SystemBusy;
    let cloned = reason.clone();
    assert_eq!(reason, cloned);
}
```

**Commit**: `feat(ubs_core): add RejectReason enum`

---

### Step 9.1.3: Define InternalOrder Struct

**File**: `src/ubs_core/order.rs`

```rust
use crate::user_account::{UserId, AssetId};
use crate::half_ulid::HalfUlid;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side { Buy, Sell }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderType { Limit, Market }

#[derive(Debug, Clone)]
pub struct InternalOrder {
    pub order_id: HalfUlid,
    pub user_id: UserId,
    pub symbol_id: u32,
    pub side: Side,
    pub price: u64,
    pub qty: u64,
    pub order_type: OrderType,
}
```

**Unit Tests**:
```rust
#[test]
fn test_side_equality() {
    assert_eq!(Side::Buy, Side::Buy);
    assert_ne!(Side::Buy, Side::Sell);
}

#[test]
fn test_order_clone() {
    let order = InternalOrder { ... };
    let cloned = order.clone();
    assert_eq!(order.user_id, cloned.user_id);
}
```

**Commit**: `feat(ubs_core): add InternalOrder struct`

---

### Step 9.1.4: Add Order Cost Calculation

**File**: `src/ubs_core/order.rs` (extend)

```rust
impl InternalOrder {
    pub fn calculate_cost(&self) -> u64 {
        match self.side {
            Side::Buy => self.price.checked_mul(self.qty).unwrap_or(u64::MAX),
            Side::Sell => self.qty,
        }
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_buy_cost_normal() {
    let order = InternalOrder { side: Side::Buy, price: 100, qty: 5, .. };
    assert_eq!(order.calculate_cost(), 500);
}

#[test]
fn test_buy_cost_overflow() {
    let order = InternalOrder { side: Side::Buy, price: u64::MAX, qty: 2, .. };
    assert_eq!(order.calculate_cost(), u64::MAX);
}

#[test]
fn test_sell_cost() {
    let order = InternalOrder { side: Side::Sell, price: 100, qty: 5, .. };
    assert_eq!(order.calculate_cost(), 5);  // qty only
}
```

**Commit**: `feat(ubs_core): add order cost calculation`

---

### Step 9.1.5: Add DebtReason Enum (Derived from EventType)

**File**: `src/ubs_core/debt.rs`

**Key Design**: DebtReason is DERIVED from EventType (no separate WAL entry needed)

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum DebtReason {
    GhostMoney,        // TradeSettle with insufficient funds
    FeeUnpaid,         // OrderFee with insufficient funds
    Liquidation,       // Forced position close deficit
    StaleSpeculative,  // Hot path credit expired
    SystemError,       // Should NEVER happen (defensive)
}

impl DebtReason {
    /// Derive debt reason from event type (exhaustive match)
    pub fn from_event_type(event_type: EventType) -> Self {
        match event_type {
            EventType::TradeSettle => DebtReason::GhostMoney,
            EventType::OrderFee => DebtReason::FeeUnpaid,
            EventType::Liquidation => DebtReason::Liquidation,
            EventType::StaleSpeculative => DebtReason::StaleSpeculative,

            // These should NEVER cause negative balance
            EventType::Deposit | EventType::OrderUnlock | EventType::Withdraw => {
                log::error!("IMPOSSIBLE: {:?} caused debt!", event_type);
                DebtReason::SystemError
            }
        }
    }
}
```

**Why Derived?**
- WAL already has EventType
- No need for separate DebtReason in WAL
- Derived on apply/replay

**Unit Tests**:
```rust
#[test]
fn test_debt_reason_from_trade_settle() {
    let reason = DebtReason::from_event_type(EventType::TradeSettle);
    assert_eq!(reason, DebtReason::GhostMoney);
}

#[test]
fn test_debt_reason_from_fee() {
    let reason = DebtReason::from_event_type(EventType::OrderFee);
    assert_eq!(reason, DebtReason::FeeUnpaid);
}

#[test]
fn test_debt_reason_impossible_event() {
    // Deposit CANNOT cause debt - should return SystemError
    let reason = DebtReason::from_event_type(EventType::Deposit);
    assert_eq!(reason, DebtReason::SystemError);
}
```

**Commit**: `feat(ubs_core): add DebtReason enum with EventType derivation`

---

### Step 9.1.6: Add DebtRecord Struct

**File**: `src/ubs_core/debt.rs` (extend)

```rust
pub struct DebtRecord {
    pub amount: u64,
    pub created_at: u64,
    pub reason: DebtReason,
    pub sequence: u64,
}
```

**Unit Tests**:
```rust
#[test]
fn test_debt_record_creation() {
    let record = DebtRecord {
        amount: 1000,
        created_at: 12345,
        reason: DebtReason::GhostMoney,
        sequence: 1,
    };
    assert_eq!(record.amount, 1000);
}
```

**Commit**: `feat(ubs_core): add DebtRecord struct`

---

### Step 9.1.7: Add DebtLedger with Basic Operations

**File**: `src/ubs_core/debt.rs` (extend)

```rust
use std::collections::HashMap;
use crate::user_account::{UserId, AssetId};

pub struct DebtLedger {
    debts: HashMap<(UserId, AssetId), DebtRecord>,
}

impl DebtLedger {
    pub fn new() -> Self {
        Self { debts: HashMap::new() }
    }

    pub fn has_debt(&self, user_id: UserId) -> bool {
        self.debts.keys().any(|(uid, _)| *uid == user_id)
    }

    pub fn get_debt(&self, user_id: UserId, asset_id: AssetId) -> Option<&DebtRecord> {
        self.debts.get(&(user_id, asset_id))
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_new_ledger_empty() {
    let ledger = DebtLedger::new();
    assert!(!ledger.has_debt(123));
}

#[test]
fn test_has_debt_after_add() {
    let mut ledger = DebtLedger::new();
    ledger.add_debt(123, 1, DebtRecord { amount: 100, .. });
    assert!(ledger.has_debt(123));
}
```

**Commit**: `feat(ubs_core): add DebtLedger basic struct`

---

### Step 9.1.8: Add DebtLedger Mutation Methods

**File**: `src/ubs_core/debt.rs` (extend)

```rust
impl DebtLedger {
    pub fn add_debt(&mut self, user_id: UserId, asset_id: AssetId, record: DebtRecord) {
        self.debts
            .entry((user_id, asset_id))
            .and_modify(|existing| existing.amount += record.amount)
            .or_insert(record);
    }

    /// Pay debt, returns remaining amount after paying
    pub fn pay_debt(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) -> u64 {
        if let Some(debt) = self.debts.get_mut(&(user_id, asset_id)) {
            let pay_off = amount.min(debt.amount);
            debt.amount -= pay_off;
            if debt.amount == 0 {
                self.debts.remove(&(user_id, asset_id));
            }
            amount - pay_off  // remaining
        } else {
            amount  // no debt, all remaining
        }
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_pay_debt_partial() {
    let mut ledger = DebtLedger::new();
    ledger.add_debt(123, 1, DebtRecord { amount: 100, .. });
    let remaining = ledger.pay_debt(123, 1, 60);
    assert_eq!(remaining, 0);
    assert_eq!(ledger.get_debt(123, 1).unwrap().amount, 40);
}

#[test]
fn test_pay_debt_full() {
    let mut ledger = DebtLedger::new();
    ledger.add_debt(123, 1, DebtRecord { amount: 100, .. });
    let remaining = ledger.pay_debt(123, 1, 150);
    assert_eq!(remaining, 50);
    assert!(ledger.get_debt(123, 1).is_none());
}

#[test]
fn test_pay_no_debt() {
    let mut ledger = DebtLedger::new();
    let remaining = ledger.pay_debt(123, 1, 100);
    assert_eq!(remaining, 100);
}
```

**Commit**: `feat(ubs_core): add DebtLedger mutation methods`

---

### Step 9.1.9: Add VipFeeTable

**File**: `src/ubs_core/fee.rs`

```rust
pub struct VipFeeTable {
    rates: [(u64, u64); 10],  // (maker_rate, taker_rate) per VIP level
}

impl VipFeeTable {
    pub fn new(rates: [(u64, u64); 10]) -> Self {
        Self { rates }
    }

    pub fn default_rates() -> Self {
        Self::new([
            (1000, 1500),  // VIP 0: 0.10%, 0.15%
            (900, 1400),   // VIP 1
            (800, 1300),   // VIP 2
            (700, 1200),   // VIP 3
            (600, 1100),   // VIP 4
            (500, 1000),   // VIP 5
            (400, 900),    // VIP 6
            (300, 800),    // VIP 7
            (200, 600),    // VIP 8
            (100, 400),    // VIP 9: 0.01%, 0.04%
        ])
    }

    pub fn get_rate(&self, vip_level: u8, is_maker: bool) -> u64 {
        let level = (vip_level as usize).min(9);
        if is_maker { self.rates[level].0 } else { self.rates[level].1 }
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_vip0_taker_rate() {
    let table = VipFeeTable::default_rates();
    assert_eq!(table.get_rate(0, false), 1500);
}

#[test]
fn test_vip9_maker_rate() {
    let table = VipFeeTable::default_rates();
    assert_eq!(table.get_rate(9, true), 100);
}

#[test]
fn test_vip_level_capped() {
    let table = VipFeeTable::default_rates();
    assert_eq!(table.get_rate(100, true), 100);  // Capped to VIP 9
}
```

**Commit**: `feat(ubs_core): add VipFeeTable`

---

### Step 9.1.9b: Add DB Value Validation (i64 ↔ u64 Safety)

**File**: `src/ubs_core/types.rs`

**🚨 WHY THIS MATTERS**:
- ScyllaDB uses `BIGINT` (signed i64)
- Rust uses `u64` (unsigned)
- Large u64 values (> i64::MAX) will appear negative when read back
- Must validate on read/write to prevent corruption

**🚨 CRITICAL RULE: NO PANIC IN PRODUCTION**
- Return `Result<T, Error>` instead of panic
- Log error and return error
- Only panic in tests (use `#[cfg(test)]`)

**📌 Responsibility Split**

| Layer | Responsibility |
|-------|----------------|
| **Asset Config** | Decimals per asset (Gateway config) |
| **Gateway** | Scale conversion, validate per-asset limits |
| **UBSCore** | Only check: value ≤ i64::MAX |

**🚨 CRITICAL: Maximum Decimals Constraint**

```
i64::MAX = 9,223,372,036,854,775,807 ≈ 9.2 × 10^18

| Decimals | Scale      | Max Balance   | Enough?     |
|----------|------------|---------------|-------------|
| 18       | 10^18      | 9.2 units     | ❌ NO!      |
| 12       | 10^12      | 9.2 million   | ⚠️ Maybe    |
| 8        | 10^8       | 92 billion    | ✅ Yes      |
| 6        | 10^6       | 9.2 trillion  | ✅ Yes      |

⚠️ ETH with 18 decimals: can only hold 9.2 ETH max!
✅ ETH with 8 decimals: can hold 92 billion ETH max
```

**Gateway MUST**:
- Configure max decimals per asset (recommend ≤ 8)
- For ETH: use 8 decimals, NOT 18
- Blockchain uses 18 decimals (wei), but CEX uses 8

**Why max 8-12 decimals?**
- Sub-cent precision: 8 decimals = 0.00000001 (enough)
- Large balances: 92 billion units fits in i64

```rust
use thiserror::Error;

/// UBSCore only needs to check this limit (for DB compatibility)
/// Decimal handling is Gateway's responsibility
pub const MAX_SAFE_VALUE: u64 = i64::MAX as u64;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DbValueError {
    #[error("Negative value in DB: {0}")]
    NegativeValue(i64),

    #[error("Value exceeds i64::MAX: {0}")]
    ExceedsMax(u64),
}

/// Convert u64 to i64 for DB storage (write)
pub fn u64_to_db(value: u64) -> Result<i64, DbValueError> {
    if value > MAX_SAFE_VALUE {
        log::error!("DB_WRITE_ERROR: value {} exceeds i64::MAX", value);
        return Err(DbValueError::ExceedsMax(value));
    }
    Ok(value as i64)
}

/// Convert i64 from DB to u64 (read)
pub fn db_to_u64(value: i64) -> Result<u64, DbValueError> {
    if value < 0 {
        log::error!("DB_READ_ERROR: negative value {} in DB", value);
        return Err(DbValueError::NegativeValue(value));
    }
    Ok(value as u64)
}
```

**🧪 Unit Tests (COMPREHENSIVE)**:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // === u64_to_db tests ===

    #[test]
    fn test_u64_to_db_zero() {
        assert_eq!(u64_to_db(0), Ok(0));
    }

    #[test]
    fn test_u64_to_db_normal_value() {
        assert_eq!(u64_to_db(1_000_000), Ok(1_000_000));
    }

    #[test]
    fn test_u64_to_db_i64_max() {
        assert_eq!(u64_to_db(MAX_SAFE_VALUE), Ok(i64::MAX));
    }

    #[test]
    fn test_u64_to_db_exceeds_max() {
        let too_large = MAX_SAFE_VALUE + 1;
        assert_eq!(u64_to_db(too_large), Err(DbValueError::ExceedsMax(too_large)));
    }

    #[test]
    fn test_u64_to_db_u64_max() {
        assert_eq!(u64_to_db(u64::MAX), Err(DbValueError::ExceedsMax(u64::MAX)));
    }

    // === db_to_u64 tests ===

    #[test]
    fn test_db_to_u64_zero() {
        assert_eq!(db_to_u64(0), Ok(0));
    }

    #[test]
    fn test_db_to_u64_normal_value() {
        assert_eq!(db_to_u64(1_000_000), Ok(1_000_000));
    }

    #[test]
    fn test_db_to_u64_i64_max() {
        assert_eq!(db_to_u64(i64::MAX), Ok(MAX_SAFE_VALUE));
    }

    #[test]
    fn test_db_to_u64_negative_one() {
        assert_eq!(db_to_u64(-1), Err(DbValueError::NegativeValue(-1)));
    }

    #[test]
    fn test_db_to_u64_min_i64() {
        assert_eq!(db_to_u64(i64::MIN), Err(DbValueError::NegativeValue(i64::MIN)));
    }

    // === Round-trip tests ===

    #[test]
    fn test_roundtrip_zero() {
        let original: u64 = 0;
        let db_value = u64_to_db(original).unwrap();
        let restored = db_to_u64(db_value).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn test_roundtrip_max() {
        let original: u64 = MAX_SAFE_VALUE;
        let db_value = u64_to_db(original).unwrap();
        let restored = db_to_u64(db_value).unwrap();
        assert_eq!(original, restored);
    }
}
```

**Error Handling in Production**:

```rust
// ✅ CORRECT: Return error, log, continue
fn load_balance(row: &Row) -> Result<Balance, Error> {
    let avail_i64: i64 = row.get("avail")?;
    let avail = db_to_u64(avail_i64)?;  // Returns Error if negative
    // ...
}

// ❌ WRONG: Don't panic in production
fn load_balance_bad(row: &Row) -> Balance {
    let avail_i64: i64 = row.get("avail").unwrap();
    assert!(avail_i64 >= 0);  // BAD: panics!
    // ...
}
```

**Commit**: `feat(ubs_core): add DB value validation (i64 ↔ u64 safety)`

### Step 9.1.10: Add DeduplicationGuard (Part 1 - Struct)

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

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn min_allowed_ts(&self) -> u64 {
        self.min_allowed_ts
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_new_guard_empty() {
    let guard = DeduplicationGuard::new();
    assert_eq!(guard.len(), 0);
    assert_eq!(guard.min_allowed_ts(), 0);
}
```

**Commit**: `feat(ubs_core): add DeduplicationGuard struct`

---

### Step 9.1.11: Add DeduplicationGuard (Part 2 - Check Logic)

**File**: `src/ubs_core/dedup.rs` (extend)

```rust
use crate::ubs_core::error::RejectReason;

impl DeduplicationGuard {
    pub fn check_and_record(&mut self, order_id: HalfUlid, now: u64) -> Result<(), RejectReason> {
        let order_ts = order_id.timestamp_ms();

        // 1. TIME CHECK - too old
        if now.saturating_sub(order_ts) > MAX_TIME_DRIFT_MS {
            return Err(RejectReason::OrderTooOld);
        }

        // 2. TIME CHECK - future
        if order_ts > now.saturating_add(1_000) {
            return Err(RejectReason::FutureTimestamp);
        }

        // 3. TIME CHECK - below min allowed
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
                self.min_allowed_ts = self.min_allowed_ts.max(evicted.timestamp_ms());
            }
        }

        // 6. INSERT
        self.cache.insert(order_id);
        Ok(())
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_accept_valid_order() {
    let mut guard = DeduplicationGuard::new();
    let order_id = HalfUlid::new(1000);  // ts=1000
    assert!(guard.check_and_record(order_id, 1000).is_ok());
}

#[test]
fn test_reject_duplicate() {
    let mut guard = DeduplicationGuard::new();
    let order_id = HalfUlid::new(1000);
    guard.check_and_record(order_id, 1000).unwrap();
    assert_eq!(
        guard.check_and_record(order_id, 1000),
        Err(RejectReason::DuplicateOrderId)
    );
}

#[test]
fn test_reject_too_old() {
    let mut guard = DeduplicationGuard::new();
    let order_id = HalfUlid::new(1000);  // ts=1000
    assert_eq!(
        guard.check_and_record(order_id, 5000),  // now=5000, diff=4000 > 3000
        Err(RejectReason::OrderTooOld)
    );
}

#[test]
fn test_reject_future() {
    let mut guard = DeduplicationGuard::new();
    let order_id = HalfUlid::new(5000);  // ts=5000
    assert_eq!(
        guard.check_and_record(order_id, 1000),  // now=1000, future by 4000 > 1000
        Err(RejectReason::FutureTimestamp)
    );
}

#[test]
fn test_eviction_updates_min_ts() {
    let mut guard = DeduplicationGuard::new();
    // Fill cache to capacity, then add one more
    // Check min_allowed_ts is updated
}
```

**Commit**: `feat(ubs_core): add DeduplicationGuard check logic`

---

### Summary: Small Steps

| Step | Description | Tests |
|------|-------------|-------|
| 9.1.1 | Module structure | `cargo check` |
| 9.1.2 | RejectReason enum | 2 tests |
| 9.1.3 | InternalOrder struct | 2 tests |
| 9.1.4 | Order cost calculation | 3 tests |
| 9.1.5 | DebtReason enum (with EventType derivation) | 3 tests |
| 9.1.6 | DebtRecord struct | 1 test |
| 9.1.7 | DebtLedger basic | 2 tests |
| 9.1.8 | DebtLedger mutations | 3 tests |
| 9.1.9 | VipFeeTable | 3 tests |
| 9.1.9b | **DB Value Validation (i64↔u64)** | **14 tests** |
| 9.1.10 | DeduplicationGuard struct | 1 test |
| 9.1.11 | DeduplicationGuard logic | 5+ tests |

**Total**: 12 small commits, 39+ unit tests

**🚨 CRITICAL RULES**:
- **NO PANIC IN PRODUCTION** - Return `Result<T, Error>` instead
- All DB read/write uses `db_to_u64()` / `u64_to_db()` helpers
- Comprehensive unit tests for edge cases

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

### Task 9.2c: Implement DebtLedger (Ghost Money Handling)

**File**: `src/ubs_core/debt.rs`

**Critical Design**: Balance stays `u64` (never negative), debt tracked separately.

```rust
/// Separate ledger for debts (ghost money scenarios)
pub struct DebtLedger {
    debts: HashMap<(UserId, AssetId), DebtRecord>,
}

pub struct DebtRecord {
    pub amount: u64,          // Debt amount (always positive)
    pub created_at: u64,      // Timestamp
    pub reason: DebtReason,   // Why debt occurred
    pub sequence: u64,        // WAL sequence for audit
}

pub enum DebtReason {
    GhostMoney,         // Trade settled without funds
    Liquidation,        // Forced position close deficit
    FeeUnpaid,          // Fee charged but no balance
    StaleSpeculative,   // Speculative credit timed out
}

impl DebtLedger {
    pub fn has_debt(&self, user_id: UserId) -> bool;
    pub fn add_debt(&mut self, user_id: UserId, asset_id: AssetId, record: DebtRecord);
    pub fn pay_debt(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) -> u64;
    pub fn total_debt(&self, user_id: UserId) -> u64;
}
```

**How It Works**:
```
Scenario: User has 0 USDT, must deduct 5000 USDT

balance.avail = 0                 // Stays 0 (saturating_sub)
debt_ledger.add(user_id, USDT, DebtRecord {
    amount: 5000,
    reason: DebtReason::GhostMoney,
})
```

**Deposit Auto-Pays Debt**:
```rust
pub fn on_deposit(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) {
    // First: Clear debt for this asset
    let remaining = self.debt_ledger.pay_debt(user_id, asset_id, amount);

    // Then: Deposit remaining to Balance
    if remaining > 0 {
        self.get_balance_mut(user_id, asset_id).deposit(remaining);
    }
}
```

**See**: [GHOST_MONEY_HANDLING.md](./GHOST_MONEY_HANDLING.md) for complete design.

**Status**: 📋 NOT STARTED

### Task 9.3: Implement Order Cost Calculation

**File**: `src/ubs_core/order.rs`

**Naming Convention (Internal vs Client)**:

| Layer | Struct Name | Values | Purpose |
|-------|-------------|--------|---------|
| **Gateway/API** | `ClientOrder` | Decimals | User-facing |
| **UBSCore** | `InternalOrder` | Raw u64 | State machine |

```rust
/// INTERNAL order - used inside UBSCore only
/// All values are raw u64 (Gateway converts decimals → u64)
#[derive(Debug, Clone)]
pub struct InternalOrder {
    pub order_id: HalfUlid,
    pub user_id: UserId,
    pub symbol_id: u32,
    pub side: Side,
    pub price: u64,      // Raw u64 (already scaled)
    pub qty: u64,        // Raw u64 (already scaled)
    pub order_type: OrderType,
}

impl InternalOrder {
    /// Calculate order cost (SECURITY: never trust Gateway's cost field)
    ///
    /// Raw u64 × u64 = u64 (no decimals, no conversion)
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

**Conversion Flow**:

```
┌─────────────────────────────────────────────────────────────────────┐
│                          CLIENT                                      │
│   { price: "50000.00", qty: "1.5" }  (decimals)                     │
└─────────────────────────────────────────────────────────────────────┘
                                │
                                │ Gateway: ClientOrder → InternalOrder
                                ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          GATEWAY                                     │
│   ClientOrder { price: "50000.00", qty: "1.5" }                     │
│   → InternalOrder { price: 5000000000000, qty: 150000000 }          │
└─────────────────────────────────────────────────────────────────────┘
                                │
                                │ InternalOrder (raw u64)
                                ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         UBSCore                                      │
│   Only sees InternalOrder - pure u64 arithmetic                     │
└─────────────────────────────────────────────────────────────────────┘
```

**Benefits**:
- **No confusion**: Clear `Internal` prefix
- **Type safety**: Compiler prevents mixing
- **Single conversion point**: Gateway only

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
use crate::user_account::{UserId, AssetId, UserAccount, Balance};

const HIGH_WATER_MARK: usize = 10_000;  // Backpressure threshold

pub struct UBSCore<R: RiskModel> {
    // State (reuse existing types!)
    accounts: HashMap<UserId, UserAccount>,
    dedup_guard: DeduplicationGuard,
    debt_ledger: DebtLedger,           // Ghost money handling
    pending_queue: VecDeque<InternalOrder>,

    // Fee configuration
    vip_configs: HashMap<UserId, u8>,  // UserId → VIP level
    vip_table: VipFeeTable,            // VIP level → rates

    // Logic
    risk_model: R,

    // Mode
    is_replay_mode: bool,
}

/// VIP fee rates (simple, stable)
pub struct VipFeeTable {
    // (maker_rate, taker_rate) - 10^6 precision (1000 = 0.1%)
    rates: [(u64, u64); 10],  // VIP 0-9
}

impl VipFeeTable {
    pub fn get_rate(&self, vip_level: u8, is_maker: bool) -> u64 {
        let level = (vip_level as usize).min(9);
        if is_maker { self.rates[level].0 } else { self.rates[level].1 }
    }
}

impl<R: RiskModel> UBSCore<R> {
    pub fn new(risk_model: R) -> Self {
        Self {
            accounts: HashMap::new(),
            dedup_guard: DeduplicationGuard::new(),
            debt_ledger: DebtLedger::new(),
            pending_queue: VecDeque::new(),
            vip_configs: HashMap::new(),
            vip_table: VipFeeTable::default(),
            risk_model,
            is_replay_mode: false,
        }
    }

    /// Process incoming order
    pub fn process_order(&mut self, order: InternalOrder) -> Result<(), RejectReason> {
        // 0. BACKPRESSURE CHECK
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

        // 5. Lock funds (use existing Balance methods)
        let asset_id = self.risk_model.get_debit_asset(&order);
        let balance = account.get_balance_mut(asset_id);
        balance.frozen(cost).map_err(|_| RejectReason::InsufficientBalance)?;

        Ok(())
    }

    /// Deposit funds (from blockchain) - pays debt first!
    pub fn on_deposit(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) {
        // First: Pay off any debt
        let remaining = self.debt_ledger.pay_debt(user_id, asset_id, amount);

        // Then: Deposit remaining to Balance
        if remaining > 0 {
            let account = self.accounts
                .entry(user_id)
                .or_insert_with(|| UserAccount::new(user_id));
            let balance = account.get_balance_mut(asset_id);
            let _ = balance.deposit(remaining);
        }
    }

    /// Calculate and apply fee (simple: always quote asset)
    fn calculate_fee(&self, user_id: UserId, trade_value: u64, is_maker: bool) -> u64 {
        let vip_level = self.vip_configs.get(&user_id).copied().unwrap_or(0);
        let rate = self.vip_table.get_rate(vip_level, is_maker);
        (trade_value * rate) / 1_000_000
    }
}
```

**Key Changes**:
- Uses existing `UserAccount`, `Balance` from `user_account.rs`
- Added `DebtLedger` for ghost money
- Added `VipFeeTable` for simple fee rates
- `on_deposit` pays debt first
- Fee calculation: simple rate lookup, no currency conversion

**Acceptance Criteria**:
- [ ] Uses existing `UserAccount`, `Balance` types
- [ ] Order processing with all checks
- [ ] Deposit pays debt first
- [ ] Fee calculation (VIP rate lookup)
- [ ] Unit tests for main flows

**Status**: 📋 NOT STARTED

---

### Task 9.7: Add EventType Enum

**File**: `src/ubs_core/types.rs`

**🚨 REQUIRED**: DebtReason.from_event_type() needs this!

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventType {
    // Can cause debt (ghost money)
    TradeSettle,
    OrderFee,
    Liquidation,
    StaleSpeculative,

    // Cannot cause debt (increases balance)
    Deposit,
    OrderUnlock,
    Withdraw,
}
```

**Unit Tests**:
```rust
#[test]
fn test_event_type_debug() {
    assert_eq!(format!("{:?}", EventType::TradeSettle), "TradeSettle");
}
```

**Commit**: `feat(ubs_core): add EventType enum`

---

### Task 9.8: Add on_trade() Method

**File**: `src/ubs_core/core.rs` (extend UBSCore)

```rust
impl<R: RiskModel> UBSCore<R> {
    /// Apply trade from Matching Engine
    /// Deducts from seller, credits to buyer
    pub fn on_trade(&mut self, trade: &InternalTrade) -> Result<(), TradeError> {
        // 1. Seller: spend frozen
        let seller = self.accounts.get_mut(&trade.seller_id)
            .ok_or(TradeError::AccountNotFound)?;
        let seller_balance = seller.get_balance_mut(trade.base_asset);
        seller_balance.spend_frozen(trade.base_qty)?;

        // 2. Buyer: apply balance delta (may create debt)
        self.apply_balance_delta(
            trade.buyer_id,
            trade.quote_asset,
            -(trade.quote_value as i64),  // Debit
            EventType::TradeSettle,
        );

        // 3. Buyer: credit base asset
        self.apply_balance_delta(
            trade.buyer_id,
            trade.base_asset,
            trade.base_qty as i64,  // Credit
            EventType::TradeSettle,
        );

        // 4. Seller: credit quote asset
        self.apply_balance_delta(
            trade.seller_id,
            trade.quote_asset,
            trade.quote_value as i64,  // Credit
            EventType::TradeSettle,
        );

        Ok(())
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_on_trade_normal() {
    // Setup: seller has 1 BTC frozen, buyer has 50000 USDT
    // Trade: 1 BTC @ 50000 USDT
    // Assert: seller gets 50000 USDT, buyer gets 1 BTC
}

#[test]
fn test_on_trade_ghost_money() {
    // Setup: buyer has 0 USDT (ghost money scenario)
    // Trade: 1 BTC @ 50000 USDT
    // Assert: buyer gets 1 BTC, debt_ledger has 50000 USDT debt
}
```

**Commit**: `feat(ubs_core): add on_trade method`

---

### Task 9.9: Add apply_balance_delta() with Debt Detection

**File**: `src/ubs_core/core.rs` (extend UBSCore)

**🚨 CRITICAL**: This is where ghost money is DETECTED and DebtLedger is populated!

```rust
impl<R: RiskModel> UBSCore<R> {
    /// Apply balance change with debt detection
    /// This is where ghost money is detected!
    fn apply_balance_delta(
        &mut self,
        user_id: UserId,
        asset_id: AssetId,
        delta: i64,
        event_type: EventType,
    ) -> u64 {
        let account = self.accounts
            .entry(user_id)
            .or_insert_with(|| UserAccount::new(user_id));
        let balance = account.get_balance_mut(asset_id);

        let current = balance.avail as i64;
        let expected = current + delta;

        if expected >= 0 {
            // Normal case: no debt
            balance.avail = expected as u64;
            return balance.avail;
        }

        // Ghost money detected!
        balance.avail = 0;
        let shortfall = (-expected) as u64;

        // Derive reason from event type (no WAL change needed!)
        let reason = DebtReason::from_event_type(event_type);

        self.debt_ledger.add_debt(user_id, asset_id, DebtRecord {
            amount: shortfall,
            reason,
            created_at: current_time_ms(),
        });

        log::warn!(
            "GHOST_MONEY: user={} asset={} shortfall={} reason={:?}",
            user_id, asset_id, shortfall, reason
        );

        0
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_apply_delta_positive() {
    let mut core = UBSCore::new(SpotRiskModel);
    core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
    assert_eq!(core.get_balance(1, 1).avail, 1000);
}

#[test]
fn test_apply_delta_negative_sufficient() {
    let mut core = UBSCore::new(SpotRiskModel);
    core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
    core.apply_balance_delta(1, 1, -500, EventType::TradeSettle);
    assert_eq!(core.get_balance(1, 1).avail, 500);
}

#[test]
fn test_apply_delta_ghost_money() {
    let mut core = UBSCore::new(SpotRiskModel);
    core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
    core.apply_balance_delta(1, 1, -5000, EventType::TradeSettle);
    // Balance clamped to 0
    assert_eq!(core.get_balance(1, 1).avail, 0);
    // Debt created
    assert!(core.debt_ledger.has_debt(1));
    assert_eq!(core.debt_ledger.get_debt(1, 1).unwrap().amount, 4000);
}
```

**Commit**: `feat(ubs_core): add apply_balance_delta with debt detection`

---

### Task 9.10: Add can_withdraw() with Debt Check

**File**: `src/ubs_core/core.rs` (extend UBSCore)

```rust
impl<R: RiskModel> UBSCore<R> {
    /// Check if user can withdraw
    /// BLOCKED if user has ANY debt
    pub fn can_withdraw(&self, user_id: UserId, asset_id: AssetId, amount: u64) -> bool {
        // 1. Block if user has ANY debt
        if self.debt_ledger.has_debt(user_id) {
            return false;
        }

        // 2. Check balance
        if let Some(account) = self.accounts.get(&user_id) {
            if let Some(balance) = account.get_balance(asset_id) {
                return balance.avail >= amount;
            }
        }

        false
    }
}
```

**Unit Tests**:
```rust
#[test]
fn test_can_withdraw_sufficient() {
    let mut core = UBSCore::new(SpotRiskModel);
    core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
    assert!(core.can_withdraw(1, 1, 500));
}

#[test]
fn test_can_withdraw_insufficient() {
    let mut core = UBSCore::new(SpotRiskModel);
    core.apply_balance_delta(1, 1, 1000, EventType::Deposit);
    assert!(!core.can_withdraw(1, 1, 2000));
}

#[test]
fn test_can_withdraw_blocked_by_debt() {
    let mut core = UBSCore::new(SpotRiskModel);
    core.apply_balance_delta(1, 1, 10000, EventType::Deposit);
    // Create debt in another asset
    core.apply_balance_delta(1, 2, -5000, EventType::TradeSettle);
    // Cannot withdraw from asset 1 because user has debt in asset 2!
    assert!(!core.can_withdraw(1, 1, 100));
}
```

**Commit**: `feat(ubs_core): add can_withdraw with debt check`

---

### Updated Summary: All Phase 9 Steps

| Step | Description | Tests |
|------|-------------|-------|
| 9.1.1 | Module structure | `cargo check` |
| 9.1.2 | RejectReason enum | 2 tests |
| 9.1.3 | InternalOrder struct | 2 tests |
| 9.1.4 | Order cost calculation | 3 tests |
| 9.1.5 | DebtReason enum (with EventType derivation) | 3 tests |
| 9.1.6 | DebtRecord struct | 1 test |
| 9.1.7 | DebtLedger basic | 2 tests |
| 9.1.8 | DebtLedger mutations | 3 tests |
| 9.1.9 | VipFeeTable | 3 tests |
| 9.1.9b | DB Value Validation | 14 tests |
| 9.1.10 | DeduplicationGuard struct | 1 test |
| 9.1.11 | DeduplicationGuard logic | 5 tests |
| **9.7** | **EventType enum** | **1 test** |
| **9.8** | **on_trade method** | **2 tests** |
| **9.9** | **apply_balance_delta** | **3 tests** |
| **9.10** | **can_withdraw** | **3 tests** |

**Total**: 16 commits, 48+ unit tests

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
├── 9.2 ✅ Balance (checked arithmetic) - EXISTING
├── 9.2b Speculative field (optional)
├── 9.2c DebtLedger (ghost money)
├── 9.3 InternalOrder + cost calculation
├── 9.4 DeduplicationGuard
├── 9.5 RiskModel Trait
├── 9.6 VipFeeTable (simple fee rates)
└── 9.7 UBSCore Struct (depends on all above)

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

1. **Task 9.1**: Crate structure
2. **Task 9.2**: ✅ EXISTING - Balance with checked arithmetic
3. **Task 9.2c**: DebtLedger (ghost money handling)
4. **Task 9.3**: InternalOrder + cost calculation
5. **Task 9.4**: DeduplicationGuard
6. **Task 9.7**: UBSCore struct
7. **Task 10.3**: Group Commit WAL

### 🟡 High Priority (Should Have)

8. **Task 9.5**: RiskModel Trait
9. **Task 9.6**: VipFeeTable
10. **Task 10.4**: WAL Replay
11. **Task 11.1-11.3**: Aeron communication

### 🟢 Future (Nice to Have)

12. **Task 9.2b**: Speculative field (if hot path needed)
13. Phase 12-13: Integration and hardening

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
