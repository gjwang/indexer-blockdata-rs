# Optimized Trade Schema Implementation Progress

**Date:** 2025-12-10
**Status:** 🚧 Phase 1 Complete - In Progress

---

## ✅ Phase 1: Schema & DB Functions (COMPLETE)

### **Created:**
1. `schema/user_trades.cql` - Optimized table schema
2. `UserTrade` struct - Data model
3. `write_user_trades()` - Dual-write function
4. `get_user_trades()` - Optimized query function
5. SQL constants for queries

### **Key Features:**
- Partition key: `(user_id, symbol_id)` ✅
- O(1) lookups ✅
- Pre-sorted by time ✅
- Denormalized (2× writes) ✅

---

## ⏳ Phase 2: Settlement Service Update (TODO)

### **Task: Enable Dual-Write**

Location: `src/bin/settlement_service.rs`

Find the trade insertion code and add:

```rust
// After inserting to settled_trades
db.insert_trades_batch(&trades, output_seq).await?;

// ADD: Also write to user_trades (optimized table)
for trade in trades {
    db.write_user_trades(trade, settled_at).await?;
}
```

**Estimated Time:** 15 minutes

---

## ⏳ Phase 3: Gateway Query Update (TODO)

### **Task: Use Optimized Query**

Location: `src/gateway.rs` - `get_trade_history()` function

Replace:
```rust
// OLD: Uses inefficient query
let trades = db.get_trades_by_user(user_id, limit).await?;
// Then filters by symbol client-side
```

With:
```rust
// NEW: Uses optimized query
let symbol_id = state.symbol_manager.get_symbol_id(&params.symbol)?;
let user_trades = db.get_user_trades(user_id, symbol_id, limit).await?;
// Convert UserTrade → DisplayTradeHistoryResponse
```

**Estimated Time:** 30 minutes

---

## ⏳ Phase 4: Backfill Script (TODO)

### **Task: Migrate Historical Data**

Create: `scripts/backfill_user_trades.rs`

```rust
// Scan settled_trades
// For each trade:
//   - Write to user_trades (buyer)
//   - Write to user_trades (seller)
```

**Estimated Time:** 1 hour
**Can run in background**

---

## ⏳ Phase 5: Testing (TODO)

### **Tasks:**
1. Run E2E test with new queries
2. Verify performance improvement
3. Compare old vs new results
4. Benchmark query times

**Estimated Time:** 30 minutes

---

## ⏳ Phase 6: Cleanup (Optional - Later)

After verification (30 days):
- Drop `settled_trades` table
- Remove old query functions
- Update all references

---

## 📊 Current Status

| Phase | Status | Time Est | Time Spent |
|-------|--------|----------|------------|
| 1. Schema + DB | ✅ Done | 30m | 30m |
| 2. Settlement Update | ⏳ TODO | 15m | - |
| 3. Gateway Update | ⏳ TODO | 30m | - |
| 4. Backfill | ⏳ TODO | 1h | - |
| 5. Testing | ⏳ TODO | 30m | - |
| **Total** | **20% Done** | **2h 45m** | **30m** |

---

## 🚀 Quick Start (Complete Phase 2)

### **Step 1: Update Settlement Service**

File: `src/bin/settlement_service.rs`

Find this section (around line 200-300):
```rust
db.insert_trades_batch(&trades, output_seq).await?;
```

Add right after:
```rust
// Dual-write to optimized table
let settled_at = get_current_timestamp_ms();
for trade in &trades {
    if let Err(e) = db.write_user_trades(trade, settled_at).await {
        log::warn!("Failed to write user_trade: {}", e);
        // Continue - not critical if this fails
    }
}
```

### **Step 2: Rebuild & Test**

```bash
cargo build --release --bin settlement_service
./test_step_by_step.sh
```

### **Step 3: Verify Dual Write**

```bash
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_trades;"
# Should show records after running tests
```

---

## 💡 Benefits Already Available

Even without completing all phases:
- ✅ Code is ready
- ✅ Schema is ready
- ✅ No breaking changes
- ✅ Can enable incrementally

---

## 📝 Notes

- **DB Table:** May need manual creation if Docker command failed
- **Symbol Mapping:** Currently hardcoded (BTC_USDT = 1), needs proper lookup
- **Dual Write:** Not critical if fails - old table still works
- **Performance:** 10-50× improvement expected when fully deployed

---

*Last Updated: 2025-12-10 20:22*
*Next Action: Complete Phase 2 (Settlement Update)*
