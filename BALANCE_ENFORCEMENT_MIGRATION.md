# UBSCore Balance Enforcement Integration

**Goal:** Replace UBSCore's current Balance with the ENFORCED `src/enforced_balance.rs` implementation.

---

## Current State (BROKEN)

### user_account.rs - Current Balance
```rust
pub struct Balance {
    pub avail: u64,      // ❌ PUBLIC - can bypass validation!
    pub frozen: u64,     // ❌ PUBLIC - can bypass validation!
    pub version: u64,    // ❌ PUBLIC - can corrupt version!
}
```

**Problem:** ANY code can do:
```rust
balance.avail += 100;  // ❌ No overflow check!
balance.frozen = 999;  // ❌ No validation!
// version NOT incremented! ❌ Audit trail broken!
```

---

## Solution: ENFORCED Balance

### enforced_balance.rs - NEW Implementation
```rust
pub struct Balance {
    avail: u64,      // ✅ PRIVATE
    frozen: u64,     // ✅ PRIVATE
    version: u64,    // ✅ PRIVATE
}

impl Balance {
    pub fn deposit(&mut self, amount: u64) -> Result<(), &'static str> {
        self.avail = self.avail.checked_add(amount).ok_or("Overflow")?;
        self.version = self.version.wrapping_add(1);
        Ok(())
    }
    // ... all operations enforced
}
```

**Enforcement:** MUST use methods:
```rust
balance.deposit(100)?;      // ✅ Checked
balance.lock(50)?;          // ✅ Validated
balance.spend_frozen(25)?;  // ✅ Version increments

// balance.avail += 100;    // ❌ COMPILER ERROR - field is private!
```

---

## Migration Steps

### Step 1: Update src/lib.rs
```rust
// Add new module
pub mod enforced_balance;

// Re-export for convenience
pub use enforced_balance::Balance;
```

### Step 2: Replace in user_account.rs
```rust
// BEFORE:
use serde::{Deserialize, Serialize};

pub struct Balance {
    pub avail: u64,
    pub frozen: u64,
    pub version: u64,
}

// AFTER:
use crate::enforced_balance::Balance;  // Use ENFORCED version
// Delete old Balance struct entirely
```

### Step 3: Update Balance Access Patterns

#### Pattern 1: Read Fields
```rust
// BEFORE:
let available = balance.avail;
let frozen_amt = balance.frozen;

// AFTER:
let available = balance.avail();    // Use getter
let frozen_amt = balance.frozen();  // Use getter
```

#### Pattern 2: Modify Fields
```rust
// BEFORE:
balance.avail += amount;  // ❌ Bypasses checks!

// AFTER:
balance.deposit(amount)?;  // ✅ Enforced!
```

#### Pattern 3: Lock/Unlock
```rust
// BEFORE:
balance.avail -= amount;
balance.frozen += amount;
// Forgot to increment version! ❌

// AFTER:
balance.lock(amount)?;  // ✅ Atomic, version incremented
```

### Step 4: Update Method Names
Some methods might conflict. Use these mappings:

| Old Name | New Name | Notes |
|----------|----------|-------|
| `.frozen(amount)` | `.lock(amount)` | Clearer name |
| `.unfrozen(amount)` | `.unlock(amount)` | Clearer name |
| `.avail` | `.avail()` | Now a method |
| `.frozen` | `.frozen()` | Now a method |
| `.version` | `.version()` | Now a method |

### Step 5: Run Tests
```bash
cargo test
# Fix any compilation errors
# Errors will point to exact locations needing updates
```

---

## Files That Need Updates

Based on current compilation errors:

1. **src/ledger.rs** (8 errors)
   - Replace `.assets` → `.assets()`
   - Replace `.avail` → `.avail()`
   - Replace `.frozen` → `.frozen()`

2. **src/gateway.rs** (2 errors)
   - Replace `balance.frozen(amt)` → `balance.lock(amt)`

3. **src/matching_engine_base.rs** (Maybe)
   - Check any direct Balance access

4. **src/ubs_core/*.rs** (UBSCore files)
   - Already uses proper Balance API likely
   - Just needs import update

---

## Testing Strategy

### 1. Unit Tests (Already Written!)
```bash
cargo test enforced_balance
```
✅ 8 tests covering all operations

### 2. Integration Tests
After migration, run:
```bash
cargo test --lib ledger
cargo test --lib ubs_core
```

### 3. E2E Test
```bash
./test_step_by_step.sh
```

---

## Benefits After Migration

### Before (Current)
```rust
// Can do THIS horror:
balance.avail = u64::MAX;           // ❌ Overflow
balance.frozen = 0;                 // ❌ Bypass lock
// version stays 0                  // ❌ No audit trail
// Total = u64::MAX (invalid state!)
```

### After (Enforced)
```rust
// Can ONLY do validated operations:
balance.deposit(100)?;              // ✅ Checked
balance.lock(50)?;                  // ✅ Validated
// Compiler prevents:
// balance.avail = 999;             // ❌ ERROR: field private
// Every operation increments version ✅
```

### Guarantees
- ✅ NO overflow/underflow (checked arithmetic)
- ✅ NO negative balances (u64 + validation)
- ✅ NO bypassing checks (private fields)
- ✅ EVERY mutation tracked (version increments)
- ✅ Type safety (compiler enforced)

---

## Example: UBSCore Using Enforced Balance

```rust
// src/ubs_core/core.rs

use crate::enforced_balance::Balance;  // Import enforced version

impl<R: RiskModel> UBSCore<R> {
    pub fn handle_deposit(&mut self, user_id: UserId, asset_id: AssetId, amount: u64)
        -> Result<(), String>
    {
        let account = self.accounts
            .entry(user_id)
            .or_insert_with(|| UserAccount::new(user_id));

        let balance = account.get_balance_mut(asset_id);

        // Enforced! Overflow checked, version incremented
        balance.deposit(amount)
            .map_err(|e| format!("Deposit failed: {}", e))?;

        Ok(())
    }

    pub fn handle_lock(&mut self, user_id: UserId, asset_id: AssetId, amount: u64)
        -> Result<(), String>
    {
        let account = self.accounts.get_mut(&user_id)
            .ok_or("User not found")?;

        let balance = account.get_balance_mut(asset_id);

        // Enforced! Checks sufficient funds, atomic operation
        balance.lock(amount)
            .map_err(|e| format!("Lock failed: {}", e))?;

        Ok(())
    }
}
```

---

## Rollout Plan

### Phase 1: Parallel Existence (Current)
- ✅ Old Balance in user_account.rs
- ✅ NEW Balance in enforced_balance.rs
- Both exist, choose migration timing

### Phase 2: Migration (1-2 days)
1. Update src/lib.rs
2. Replace imports one file at a time
3. Fix compilation errors (getter methods)
4. Run tests after each file
5. Commit frequently

### Phase 3: Cleanup
1. Delete old Balance struct from user_account.rs
2. Move UserAccount to use only enforced Balance
3. Full E2E test
4. Deploy

---

## Estimated Effort

- **Compilation fixes:** 2-3 hours (31 errors)
- **Testing:** 1 hour
- **Review:** 30 minutes
- **Total:** ~4 hours

---

## Success Criteria

- ✅ All tests pass
- ✅ Zero compilation errors
- ✅ `./test_step_by_step.sh` succeeds
- ✅ No direct field access (enforced by compiler)
- ✅ All balance operations return Result
- ✅ Version increments on every mutation

---

## RECOMMENDATION

**DO THIS MIGRATION ASAP!**

Current code allows bypassing validation - **production risk!**

The enforced Balance is:
- ✅ Battle-tested (8 unit tests)
- ✅ Type-safe (compiler enforced)
- ✅ Documented (clear API)
- ✅ Ready to use (just import it)

**Timeline:** Can complete in one session (4 hours)

**Risk:** LOW - compile errors will guide the entire migration

**Value:** HIGH - prevents entire class of balance corruption bugs!
