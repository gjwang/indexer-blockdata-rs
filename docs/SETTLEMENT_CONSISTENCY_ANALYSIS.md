# Settlement Service Consistency Analysis

## Overview
This document analyzes the Settlement Service's data consistency guarantees for trade history, order history, and user balances.

## Current Architecture

### Data Flow
```
Matching Engine (ZMQ) → Settlement Service → ScyllaDB
                                          ↓
                                     StarRocks (async)
                                          ↓
                                     CSV Backup
```

### Critical Tables
1. **`settled_trades`** - Trade history (source of truth)
2. **`user_balances`** - User balance state (materialized view)
3. **`ledger_events`** - Deposit/withdrawal events

---

## Consistency Analysis

### ✅ **STRENGTHS**

#### 1. **Trade History Durability**
- **Primary Key**: `trade_id` (unique, immutable)
- **Idempotency**: Trade inserts use `trade_id` as primary key
  - Duplicate inserts will fail (ScyllaDB primary key constraint)
  - No data loss if settlement service restarts
- **Retry Logic**: 3 retries with exponential backoff
- **Fallback**: Failed trades logged to `failed_trades.json`
- **Audit Trail**: CSV backup for disaster recovery

**Verdict**: ✅ **Trade history is consistent and durable**

#### 2. **Balance Update Idempotency**
- **Version-Based LWT**: Uses Lightweight Transactions (IF version = ?)
- **Matching Engine Versions**: Each balance update has a version from ME
- **Idempotent**: Re-processing same trade won't double-update balances
  ```rust
  if balance.version >= expected_version {
      return Ok(false); // Already processed
  }
  ```

**Verdict**: ✅ **Balance updates are idempotent**

#### 3. **Sequence Gap Detection**
- Tracks `output_sequence` from matching engine
- Logs critical errors if gaps detected
- Allows manual intervention/replay

**Verdict**: ✅ **Gap detection works**

---

### ⚠️ **WEAKNESSES & RISKS**

#### 1. **❌ CRITICAL: Non-Atomic Trade + Balance Updates**

**Problem**:
```rust
// Line 197-205: These are TWO separate operations
match settlement_db.insert_trade(&trade).await {
    Ok(()) => {
        // ⚠️ If this fails, trade is persisted but balances are NOT updated
        if let Err(e) = update_balances_for_trade(&settlement_db, &trade).await {
            log::error!("Failed to update balances...");
            // Continue processing - balance can be rebuilt from trades
        }
    }
}
```

**Scenario**:
1. `insert_trade()` succeeds → Trade is in `settled_trades`
2. `update_balances_for_trade()` fails → Balances NOT updated
3. **Result**: Trade history exists, but user balances are WRONG

**Impact**:
- User sees trade in history
- User balance doesn't reflect the trade
- **Data inconsistency**

**Current Mitigation**: Comment says "balance can be rebuilt from trades" but:
- ❌ No automatic rebuild mechanism exists
- ❌ Requires manual intervention
- ❌ Users see incorrect balances until rebuild

---

#### 2. **❌ CRITICAL: Partial Balance Updates**

**Problem**:
```rust
// Lines 340-376: Four separate LWT operations
let buyer_quote_updated = db.update_balance_with_version(...).await?;  // 1
let buyer_base_updated = db.update_balance_with_version(...).await?;   // 2
let seller_base_updated = db.update_balance_with_version(...).await?;  // 3
let seller_quote_updated = db.update_balance_with_version(...).await?; // 4
```

**Scenario**:
1. Buyer quote update succeeds (buyer loses USDT)
2. Buyer base update **FAILS** (buyer doesn't gain BTC)
3. **Result**: Buyer lost USDT but didn't get BTC!

**Impact**:
- **Money disappears** from the system
- Buyer is financially harmed
- Requires manual reconciliation

**Current Mitigation**:
- Version-based LWT prevents double-application
- But doesn't prevent partial application

---

#### 3. **⚠️ Balance Rebuild Not Implemented**

**Problem**:
- Code comments say "balance can be rebuilt from trades"
- **No rebuild function exists**
- Manual SQL queries required

**Impact**:
- Operational burden
- Downtime during reconciliation
- Risk of human error

---

#### 4. **⚠️ Order History Derived from Trades**

**Problem**:
- No dedicated `orders` table
- Order history is aggregated from `settled_trades` at query time
- Only shows **filled** orders (no partial fills, no cancelled orders)

**Impact**:
- Incomplete order history
- Can't show order lifecycle (placed → partial fill → cancelled)
- Performance: Aggregation on every query

---

## Recommendations

### **Priority 1: CRITICAL - Fix Atomicity**

#### Option A: **Two-Phase Commit (Recommended)**
```rust
// 1. Write trade + balance updates in a BATCH
let batch = BatchStatement::new();
batch.append(insert_trade_stmt);
batch.append(update_buyer_quote_stmt);
batch.append(update_buyer_base_stmt);
batch.append(update_seller_base_stmt);
batch.append(update_seller_quote_stmt);
session.batch(&batch, ()).await?;
```

**Pros**:
- Atomic: All or nothing
- ScyllaDB native support
- No code complexity

**Cons**:
- Can't use LWT in batches (ScyllaDB limitation)
- Need alternative idempotency mechanism

#### Option B: **Compensating Transactions**
```rust
match update_balances_for_trade(&settlement_db, &trade).await {
    Err(e) => {
        // Rollback: Delete the trade
        settlement_db.delete_trade(trade.trade_id).await?;
        log::error!("Rolled back trade {}", trade.trade_id);
    }
}
```

**Pros**:
- Keeps LWT for idempotency
- Eventual consistency

**Cons**:
- Complex rollback logic
- Trade might be visible briefly

#### Option C: **Balance Rebuild Service (Current Approach + Automation)**
```rust
// Periodic reconciliation job
async fn rebuild_balances_from_trades(db: &SettlementDb) {
    // 1. Get all trades
    // 2. Recalculate balances
    // 3. Compare with current balances
    // 4. Fix discrepancies
}
```

**Pros**:
- Keeps current architecture
- Automated recovery

**Cons**:
- Temporary inconsistency
- Complex reconciliation logic

---

### **Priority 2: Add Balance Rebuild Function**

```rust
impl SettlementDb {
    pub async fn rebuild_user_balance(
        &self,
        user_id: u64,
        asset_id: u32
    ) -> Result<()> {
        // 1. Get all trades for user
        let trades = self.get_trades_by_user(user_id, i32::MAX).await?;

        // 2. Calculate balance from trades
        let mut balance = 0i64;
        for trade in trades {
            if trade.buyer_user_id == user_id && trade.base_asset == asset_id {
                balance += trade.quantity as i64;
            }
            // ... (handle all cases)
        }

        // 3. Update balance (force update, ignore version)
        self.force_update_balance(user_id, asset_id, balance).await?;
    }
}
```

---

### **Priority 3: Add Order Lifecycle Tracking**

Create dedicated `orders` table:
```sql
CREATE TABLE orders (
    user_id bigint,
    order_id bigint,
    symbol text,
    side text,
    price bigint,
    quantity bigint,
    filled_quantity bigint,
    status text,  -- PLACED, PARTIAL, FILLED, CANCELLED
    created_at bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, order_id)
);
```

---

## Current Guarantees

| Property | Status | Notes |
|----------|--------|-------|
| **Trade History Durability** | ✅ STRONG | Idempotent, retry logic, fallback |
| **Trade History Completeness** | ⚠️ WEAK | Gaps possible, manual replay needed |
| **Balance Consistency** | ❌ **WEAK** | Non-atomic updates, partial failures possible |
| **Balance Idempotency** | ✅ STRONG | Version-based LWT |
| **Order History** | ⚠️ LIMITED | Only filled orders, no lifecycle |

---

## Recommended Action Plan

1. **Immediate** (This Week):
   - [ ] Implement balance rebuild function
   - [ ] Add monitoring for balance update failures
   - [ ] Document manual reconciliation procedure

2. **Short Term** (This Month):
   - [ ] Implement Option A (Batch) or Option B (Compensating Transactions)
   - [ ] Add automated balance reconciliation job
   - [ ] Add integration tests for failure scenarios

3. **Long Term** (Next Quarter):
   - [ ] Add `orders` table for full order lifecycle
   - [ ] Implement automatic gap replay from matching engine
   - [ ] Add real-time balance verification

---

## Conclusion

**Current State**:
- ✅ Trade history is **durable and consistent**
- ❌ User balances have **atomicity issues**
- ⚠️ Order history is **incomplete**

**Risk Level**: **MEDIUM-HIGH**
- Production-ready for read-heavy workloads
- **NOT production-ready** for financial accuracy without manual reconciliation

**Next Steps**: Implement Priority 1 (atomicity fix) before production deployment.
