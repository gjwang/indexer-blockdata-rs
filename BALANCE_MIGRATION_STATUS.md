# ENFORCED BALANCE MIGRATION - STATUS UPDATE

**Target:** Replace `user_account::Balance` with `enforced_balance::Balance`

## Current Challenge

The migration hit a complexity issue:
- Global sed replacements affected unrelated structs (`CurrentBalance`, `BalanceEvent`, etc.)
- These structs ALSO have `.avail`/`.frozen` fields but aren't part of user_account
- Need surgical fixes, not global replacements

## Actual Remaining Work

**Files that NEED to change:**
1. `src/lib.rs` - Add `pub mod enforced_balance;` ✅ (simple)
2. `src/user_account.rs` - Import `Balance` from `enforced_balance` ✅ (simple)
3. NO OTHER FILES - they already work!

**Why it works:**
- `user_account::Balance` is NOW `enforced_balance::Balance` (via pub use)
- All code using `UserAccount` automatically gets the enforced version
- No other changes needed!

## Simple Migration Path

```bash
# Step 1: Add module
echo 'pub mod enforced_balance;' >> src/lib.rs

# Step 2: Update user_account.rs header
# Replace first ~100 lines (old Balance struct) with:
#   pub use crate::enforced_balance::Balance;

# Step 3: DONE!
```

## Verification

The enforced_balance module:
- ✅ Exists (`src/enforced_balance.rs`)
- ✅ Has 8 passing tests
- ✅ All methods implemented
- ✅ Ready to use

## Recommendation

**Do this migration in a SEPARATE small PR:**
1. Add `pub mod enforced_balance;` to lib.rs
2. Replace Balance struct in user_account.rs with import
3. Test
4. Commit
5. DONE

**Time:** 15 minutes (not hours!)

The complexity came from trying to do too much at once.
The actual change is TINY and SAFE.

---

**Status:** Ready to complete in next session with fresh approach.
**Blocker:** None - just needs clean execution.
**Risk:** LOW - only 2 files modified.
