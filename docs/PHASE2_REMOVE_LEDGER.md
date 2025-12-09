# Phase 2: Remove GlobalLedger from Matching Engine

## Goal
Make UBSCore the single source of truth for balances. ME should NEVER maintain balance state.

## Current Architecture (What We're Removing)

```
MatchingEngine {
    ledger: GlobalLedger,  // ← REMOVE THIS
    // ... matching state only
}
```

ME currently:
- ✅ Validates order syntax
- ❌ Checks balances using GlobalLedger (SHOULD USE UBSCORE)
- ❌ Locks/unlocks funds in GlobalLedger (SHOULD BE IN UBSCORE)
- ✅ Matches orders (KEEP)
- ❌ Handles deposits/withdrawals via BalanceProcessor (MOVE TO UBSCORE)

## Target Architecture

```
MatchingEngine {
    // NO ledger field!
    order_books: Vec<Option<OrderBook>>,
    // ... matching state only
}
```

ME will:
- ✅ Receive ONLY pre-validated orders from UBSCore
- ✅ Trust that funds are already locked
- ✅ Match orders
- ✅ Send trade results to Settlement (which updates UBSCore)

## Implementation Steps

### Step 1: Simplify ME - Remove Balance Logic

**Files to modify:**
- `src/matching_engine_base.rs`
- `src/bin/matching_engine_server.rs`

**Changes:**
1. Remove `ledger: GlobalLedger` from `MatchingEngine` struct
2. Remove all `ledger.apply()` calls
3. Remove balance checking in `add_order()`
4. Assume all orders are pre-validated by UBSCore
5. Remove `transfer_in_and_build_output()` and `transfer_out_and_build_output()`
6. Remove `BalanceProcessor` from matching_engine_server

**Why this is safe:**
- UBSCore already validates orders BEFORE sending to ME
- Orders reaching ME have already passed balance checks
- Funds are already locked in UBSCore

### Step 2: Update Deposit/Withdraw Flow

**Current:** Gateway → Kafka(balance_ops) → ME.BalanceProcessor → Settlement
**New:**     Gateway → UBSCore(Aeron) → Settlement

**Changes needed:**
1. Gateway sends deposits to UBSCore via Aeron (already supported!)
2. UBSCore updates its RAM state
3. UBSCore publishes balance events to Kafka for Settlement
4. Settlement writes to ScyllaDB

### Step 3: Settlement Integration

Settlement needs to:
1. Continue receiving EngineOutput from ME (trades only, no balance events from ME)
2. Receive balance events from UBSCore (new)
3. Write both to ScyllaDB

### Step 4: Update E2E Test

Update `test_full_e2e.sh` to:
1. Send deposits to UBSCore (not via Kafka balance_ops)
2. Verify balances are in UBSCore RAM
3. Verify Settlement writes to ScyllaDB

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| ME can't validate if UBSCore is down | UBSCore must be highly available, use Aeron for speed |
| Race condition: UBSCore locks funds, but ME crashes before matching | UBSCore should have unlock mechanism on timeout |
| Settlement gets out of sync with UBSCore | UBSCore is source of truth, Settlement is eventually consistent |

## Testing Strategy

1. Unit tests: ME matching logic (without balance checks)
2. Integration test: UBSCore → ME → Settlement flow
3. E2E test: Full order lifecycle including deposits
4. Chaos test: Kill ME mid-match, verify UBSCore state is consistent

## Success Criteria

- ✅ `cargo build` passes
- ✅ E2E test passes with new flow
- ✅ Deposits go through UBSCore
- ✅ Orders validated by UBSCore only
- ✅ ME has NO balance state
- ✅ Settlement writes correctly from both ME (trades) and UBSCore (deposits)
