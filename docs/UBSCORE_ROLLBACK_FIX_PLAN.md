# UBSCore Rollback Fix Plan

**Date**: 2025-12-14
**Status**: 🔴 CRITICAL - Must Fix Before Production
**Issue**: `UbsTradingAdapter.rollback()` returns `Success` when it should return `Failed`

---

## Problem Statement

```rust
// CURRENT (WRONG):
async fn rollback(&self, req_id: RequestId) -> OpResult {
    log::warn!("UbsTradingAdapter::rollback({}) - not implemented", req_id);
    OpResult::Success  // ❌ WRONG! Pretends it worked!
}
```

### Why This Is Wrong

1. **UBSCore operations are final** - cannot be rolled back
2. **Returning Success is a lie** - no rollback happened
3. **FSM will think rollback succeeded** - moves to RolledBack state
4. **Money is actually LOST** - debited from Trading but never credited back

---

## What Should Happen

```rust
// CORRECT:
async fn rollback(&self, req_id: RequestId) -> OpResult {
    log::error!(
        "FORBIDDEN: UbsTradingAdapter::rollback({}) called! \
         UBSCore operations are final and cannot be rolled back.",
        req_id
    );
    OpResult::Failed("FORBIDDEN: UBSCore operations are final".to_string())
}
```

### Why Failed is Correct

1. **Clearly indicates rollback is not possible**
2. **FSM stays in Compensating state** - doesn't falsely transition
3. **Alerts ops** - error log triggers investigation
4. **Honest about the situation** - money needs manual intervention

---

## Root Cause Analysis

The deeper issue is: **Trading→Funding transfers should NEVER reach Compensating state**

### FSM Behavior Verified (coordinator.rs lines 297-318)

```rust
async fn step_compensating(...) {
    let result = source.rollback(req_id).await;

    match result {
        OpResult::Success => {
            // Transitions to RolledBack ❌ (money lost!)
        }
        OpResult::Failed(e) => {
            // Stays in Compensating, keeps retrying
            // This is what we WANT for UBSCore!
        }
        OpResult::Pending => {
            // Stays in Compensating
        }
    }
}
```

### Why `OpResult::Failed` is Correct for UBSCore

When `rollback()` returns `Failed`:
1. FSM stays in `Compensating` state
2. Worker keeps retrying rollback
3. Alert is logged for ops investigation
4. Transfer is stuck until manual intervention

This is the **correct behavior** for funds that cannot be rolled back - they require human review!

### Current Flow (Problematic)

```
Trading→Funding:
1. Trading.withdraw() → Success (money debited from RAM)
2. Funding.deposit() → Failed (TB error)
3. FSM → Compensating
4. Trading.rollback() → Success (but does nothing!)
5. FSM → RolledBack
6. ❌ Money LOST! (debited from Trading, never credited to Funding)
```

### Required Design Change

For Trading source transfers:
- Target operations should **NEVER fail** (retry forever)
- OR define explicit behavior for "stuck" transfers

---

## Fix Plan

### Phase 1: Fix Rollback Return Value (Immediate)

```rust
// File: src/transfer/adapters/trading.rs
// Line: ~372-376

async fn rollback(&self, req_id: RequestId) -> OpResult {
    log::error!(
        "FORBIDDEN: UbsTradingAdapter::rollback({}) - UBSCore operations are final",
        req_id
    );
    OpResult::Failed("FORBIDDEN: UBSCore operations are final".to_string())
}
```

### Phase 2: FSM Design Review

Need to verify:
1. When Trading is source, does target ever transition to `Compensating`?
2. Should we add a check to prevent `Compensating` for Trading source?
3. What happens when rollback returns `Failed`?

### Phase 3: Query Status

Implement proper status query:
```rust
async fn query(&self, req_id: RequestId) -> OpResult {
    // Query UBSCore for the actual status of this transfer
    match self.client.query_transfer_status(req_id).await {
        Ok(status) if status.is_success() => OpResult::Success,
        Ok(status) if status.is_failed() => OpResult::Failed(status.reason()),
        _ => OpResult::Pending,
    }
}
```

---

## Verification Checklist

- [ ] Change `rollback()` to return `OpResult::Failed(...)`
- [ ] Add integration test for Trading→Funding with failing target
- [ ] Verify FSM behavior when rollback fails
- [ ] Add alerting for FORBIDDEN rollback attempts
- [ ] Update documentation

---

## Files to Modify

| File | Change |
|------|--------|
| `src/transfer/adapters/trading.rs` | Line 372-376: Change return to `OpResult::Failed` |
| `src/transfer/coordinator.rs` | Review `step_compensating()` behavior |
| `docs/INTERNAL_TRANSFER_AUDIT_REPORT.md` | Update with correct findings |

---

**Approved for Implementation**: YES / NO
**Reviewed By**: [Pending]
