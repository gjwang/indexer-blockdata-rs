# Phase 2 Progress: ME GlobalLedger Removal - IN PROGRESS

## Status: COMPILATION ERRORS (19 errors to fix)

### ✅ Completed So Far

1. **Removed `ledger: GlobalLedger` field** from `MatchingEngine` struct
   - Located in `src/matching_engine_base.rs:245`

2. **Removed ledger initialization** from `MatchingEngine::new()`
   - Removed `GlobalLedger::new()` call
   - Removed account loading from snapshots
   - Removed ledger_seq tracking

3. **Removed transfer methods**
   - Deleted `transfer_in_and_build_output()` (116 lines)
   - Deleted `transfer_out_and_build_output()` (116 lines)
   - These are now UBSCore's responsibility

### 🔧 Remaining Work: Fix 19 Compilation Errors

All errors are `no field ledger on type MatchingEngine`. They fall into categories:

#### Category 1: Balance Checking (DELETE - Trust UBSCore)
- Line 444: `self.ledger.get_accounts()` - checking balance before order
- **Fix**: REMOVE this check entirely. UBSCore already validated.

#### Category 2: Ledger Operations in process_order_logic() (REMOVE - No balance ops)
- Line 470, 533: Passing `&mut self.ledger` to process_order_logic
- **Fix**: Remove ledger parameter from function signature

#### Category 3: Ledger Flush (DELETE - No ledger to flush)
- Line 500, 555: `self.ledger.flush()`
- **Fix**: Remove these calls

#### Category 4: Shadow Ledger (DELETE - Batch processing artifact)
- Line 1297, 1325: Creating shadow ledger for batch atomicity
- **Fix**: Remove shadow ledger entirely (was only needed for balance state)

#### Category 5: Tests (FIX LATER)
- Lines 1483-1522: Test code using transfer_in_to_trading_account
- **Fix**: Update tests or disable temporarily

## Next Steps (In Order)

### Step 1: Remove Balance Checking in add_order()

**File**: `src/matching_engine_base.rs`
**Lines**: 444-458

```rust
// REMOVE THIS BLOCK:
let accounts = self.ledger.get_accounts();
let balance = accounts
    .get(&user_id)
    .and_then(|user| user.assets.iter().find(|(a, _)| *a == required_asset))
    .map(|(_, b)| b.avail)
    .unwrap_or(0);

if balance < required_amount {
    return Err(OrderError::InsufficientFunds { ... });
}
```

**Replace with:**
```rust
// Balance validation is done by UBSCore - trust that funds are locked
```

### Step 2: Remove Ledger from process_order_logic()

The function `process_order_logic()` currently takes `ledger: &mut impl Ledger` parameter.

**Changes needed:**
1. Remove `ledger` parameter from function signature
2. Remove all `ledger.apply()`, `ledger.get_balance()` calls inside
3. ME should only manipulate order books, NOT balances

**Philosophy**: ME creates BalanceEvents in EngineOutput, but doesn't apply them itself.

### Step 3: Remove Ledger Flush Calls

Lines 500, 555: `self.ledger.flush()`

Simply delete these lines.

### Step 4: Remove Shadow Ledger for Batch Processing

Lines 1297, 1325: Shadow ledger was used for atomic batch balance updates.

Since ME no longer has balance state, remove:
- `ShadowLedger::new(&self.ledger)`
- `self.ledger.apply_delta_to_memory(delta)`

### Step 5: Fix or Disable Tests

Test code at lines 1483-1522 uses methods we deleted.

**Options:**
A) Comment out these tests temporarily
B) Rewrite to use UBSCore client
C) Delete if obsolete

**Recommendation**: Comment out for now, fix after main code compiles.

### Step 6: Remove BalanceProcessor from matching_engine_server.rs

**File**: `src/bin/matching_engine_server.rs`
**Line**: 165: `engine.transfer_in_and_build_output()`

This entire BalanceProcessor needs to be removed:
- Delete `struct BalanceProcessor`
- Delete balance_topic subscription
- Remove EngineCommand::BalanceRequest handling

## Expected End State

**MatchingEngine will:**
- ✅ Receive only PRE-VALIDATED orders from UBSCore
- ✅ Match orders in order books
- ✅ Generate trades
- ❌ NOT check balances
- ❌ NOT lock/unlock funds
- ❌ NOT handle deposits/withdrawals

**UBSCore will:**
- ✅ Validate orders
- ✅ Lock funds
- ✅ Forward validated orders to ME
- ✅ Handle deposits/withdrawals
- ✅ Be the single source of truth for balances

## Testing Strategy After Compilation

1. Unit test: ME matching without balance checks
2. Integration test: UBSCore → ME flow
3. E2E test: Full order + deposit flow
4. Verify: UBSCore state remains consistent

## Current Compilation Error Count

```bash
cargo check 2>&1 | grep "error\[" | wc -l
# Output: 19
```

## Files Modified in This Phase

- `src/matching_engine_base.rs` - Main ME logic (IN PROGRESS)
- `src/bin/matching_engine_server.rs` - Server (TODO)
- `test_full_e2e.sh` - E2E test (TODO - may need updates)

## Estimated Time to Complete

- Fix 19 compilation errors: ~30-45 minutes
- Remove BalanceProcessor: ~15 minutes
- Update tests: ~30 minutes
- E2E verification: ~15 minutes

**Total**: ~1.5-2 hours of focused work

## Risk Assessment

✅ **Low Risk**: We're early in development, no production system
✅ **Clean Break**: Old architecture clearly separated from new
⚠️ **Testing Critical**: Must verify UBSCore ↔ ME integration works

## Rollback Plan

If this doesn't work, we can:
1. `git reset --hard HEAD~1` to previous commit
2. Re-evaluate architecture
3. Consider Option A (gradual migration) instead

But given we're pre-production, pushing through is the right call.
