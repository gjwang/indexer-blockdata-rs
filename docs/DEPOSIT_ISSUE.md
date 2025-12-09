# Issue: Deposits Not Reaching UBSCore

## Problem
E2E test shows **0 deposits** written to ScyllaDB.

## Root Cause
Gateway sends deposits to Kafka `balance.operations` topic, but **nothing consumes it**:
- ME's BalanceProcessor was removed (Phase 2)
- UBSCore doesn't have a Kafka consumer for balance operations

## Current Flow (BROKEN)
```
Gateway
  → transfer_in endpoint
    → Kafka balance.operations topic
      → **NOBODY LISTENING** ❌
```

## Expected Flow (Phase 2 Architecture)
```
Gateway
  → Deposits go to UBSCore
    → UBSCore updates balance in RAM
      → UBSCore emits event to Kafka
        → Settlement writes to ScyllaDB ✅
```

## Solution Options

### Option A: UBSCore Kafka Consumer (RECOMMENDED)
**Pro**: Minimal Gateway changes, reuses existing Kafka infrastructure
**Con**: Adds Kafka dependency to UBSCore

**Implementation**:
1. Add Kafka consumer to UBSCore for `balance.operations` topic
2. Deserialize `BalanceRequest::TransferIn/TransferOut`
3. Call `handle_deposit()` / `handle_withdraw()`
4. Emit balance events to Settlement

### Option B: Gateway Uses Aeron
**Pro**: Direct communication, lower latency
**Con**: Requires Gateway refactor, Aeron feature flag complications

**Implementation**:
1. Update `transfer_in()` to use `ubs_client.send_deposit()`
2. Remove Kafka balance topic
3. Ensure aeron feature is enabled

### Option C: Hybrid (Current + Fix)
Gateway sends to Kafka → UBSCore consumes → Updates balance → Emits to Settlement

This leverages existing infrastructure and is quickest to implement.

## Recommendation

**Implement Option A** - Add Kafka consumer to UBSCore

### Why?
1. **Minimal changes**: Gateway code unchanged
2. **Decoupled**: UBSCore can process deposits from multiple sources
3. **Observable**: Kafka topics are easy to monitor/replay
4. **Consistent**: Matches how orders flow (Kafka-based)

## Implementation Steps

1. Add `rdkafka` consumer to `ubscore_aeron_service`
2. Subscribe to `balance.operations` topic
3. Deserialize `BalanceRequest` messages
4. Route to `handle_deposit()` / `handle_withdraw()`
5. Emit balance events via Kafka producer (for Settlement)

## Testing
After fix, E2E test should show:
- ✅ 9 deposits in balance_ledger (3 users × 3 assets)
- ✅ Balances visible via Gateway API
- ✅ Settlement Service receives events

---

**Current Status**: Identified root cause
**Next Step**: Implement UBSCore Kafka consumer for deposits
