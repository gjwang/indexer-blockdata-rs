# UNLOGGED Batch - Performance vs Safety Trade-off

## What is UNLOGGED Batch?

```rust
// UNLOGGED batch (fast but no atomicity guarantee)
let mut batch = BatchStatement::new(BatchType::Unlogged);
```

**What it does**:
- ✅ **No batch log** → No extra disk I/O
- ✅ **Fast** → Similar performance to individual writes
- ❌ **No atomicity guarantee** across crashes

---

## UNLOGGED Batch Guarantees

### What UNLOGGED Provides

**Within a single execution**:
- ✅ All statements sent to ScyllaDB together
- ✅ If no crash, all statements execute
- ✅ **Best-effort atomicity**

**Across crashes**:
- ❌ **No guarantee** - some statements may succeed, others may fail
- ❌ **No replay** - no batch log to recover from

### Example Scenario

```rust
let mut batch = BatchStatement::new(BatchType::Unlogged);
batch.append("INSERT INTO settled_trades ...");  // Statement 1
batch.append("UPDATE user_balances ...");        // Statement 2
batch.append("UPDATE user_balances ...");        // Statement 3
batch.append("UPDATE user_balances ...");        // Statement 4
batch.append("UPDATE user_balances ...");        // Statement 5

session.batch(&batch, ()).await?;
```

**If ScyllaDB crashes during execution**:
- ✅ Statement 1, 2, 3 may succeed
- ❌ Statement 4, 5 may fail
- ❌ **Partial state** - trade inserted but not all balances updated

**If app crashes before batch**:
- ❌ Nothing written (same as LOGGED)

**If app crashes after batch sent**:
- ⚠️ **Unknown** - may be partially applied

---

## When is UNLOGGED Safe?

### Safe Use Case 1: Idempotent Operations

If all statements are **naturally idempotent**:

```rust
// Safe - all statements are idempotent inserts
let mut batch = BatchStatement::new(BatchType::Unlogged);

for event in events {
    // Each event has unique ID (primary key)
    // Duplicate inserts will fail harmlessly
    batch.append("INSERT INTO events (id, data) VALUES (?, ?)", (event.id, event.data));
}

session.batch(&batch, ()).await?;
```

**Why safe**:
- If batch partially fails, just retry
- Duplicate inserts are rejected by primary key
- No inconsistent state possible

### Safe Use Case 2: Independent Operations

If statements are **completely independent**:

```rust
// Safe - each user update is independent
let mut batch = BatchStatement::new(BatchType::Unlogged);

for user in users {
    // Each user's data is independent
    batch.append("UPDATE user_profiles SET last_login = ? WHERE user_id = ?",
                 (now(), user.id));
}

session.batch(&batch, ()).await?;
```

**Why safe**:
- Partial failure doesn't create inconsistency
- Each user's state is independent
- Can retry failed users separately

### ❌ UNSAFE Use Case: Dependent Operations (Settlement!)

```rust
// ❌ UNSAFE - statements are dependent!
let mut batch = BatchStatement::new(BatchType::Unlogged);

batch.append("INSERT INTO settled_trades ...");  // Trade
batch.append("UPDATE user_balances SET available = available - 100 WHERE user_id = 1");  // Buyer USDT
batch.append("UPDATE user_balances SET available = available + 1 WHERE user_id = 1");    // Buyer BTC
batch.append("UPDATE user_balances SET available = available - 1 WHERE user_id = 2");    // Seller BTC
batch.append("UPDATE user_balances SET available = available + 100 WHERE user_id = 2");  // Seller USDT

session.batch(&batch, ()).await?;
```

**Why unsafe**:
- If trade inserts but only 2 balance updates succeed:
  - ❌ Buyer lost USDT but didn't gain BTC
  - ❌ Seller's balances unchanged
  - ❌ **Money disappeared from system!**

---

## Can We Make UNLOGGED Safe for Settlement?

### Option 1: Application-Level Atomicity (WAL)

Add **application WAL** to provide atomicity:

```rust
pub async fn settle_trade_unlogged_safe(
    db: &SettlementDb,
    wal: &mut SettlementWal,
    trade: &MatchExecData,
) -> Result<()> {
    // 1. Write to WAL first (durable, atomic)
    wal.append(trade).await?;
    wal.fsync().await?;  // Ensure durable

    // 2. Execute UNLOGGED batch (fast)
    let mut batch = BatchStatement::new(BatchType::Unlogged);
    batch.append(insert_trade_stmt(trade));
    batch.append(update_balance_stmt(/* ... */));
    // ... (4 balance updates)

    db.session.batch(&batch, ()).await?;

    // 3. Mark as committed in WAL
    wal.mark_committed(trade.output_sequence).await?;

    Ok(())
}

// On startup: Recover from WAL
pub async fn recover_from_wal(db: &SettlementDb, wal: &SettlementWal) -> Result<()> {
    let pending = wal.get_pending_trades().await?;

    for trade in pending {
        // Check what was written
        let trade_exists = db.trade_exists(trade.trade_id).await?;

        if trade_exists {
            // Trade inserted, verify balances
            let balances_ok = verify_balances_for_trade(db, &trade).await?;

            if !balances_ok {
                // Fix balances
                fix_balances_for_trade(db, &trade).await?;
            }
        } else {
            // Nothing written, retry entire settlement
            settle_trade_unlogged_safe(db, wal, &trade).await?;
        }
    }

    Ok(())
}
```

**Performance**:
- ✅ **Fast** (UNLOGGED batch)
- ✅ **Atomic** (WAL provides durability)
- ⚠️ **Complex** (need WAL + recovery logic)

### Option 2: Idempotent Design

Make balance updates **idempotent** using versions:

```rust
// Store expected balance in trade record
CREATE TABLE settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
    buyer_user_id bigint,
    seller_user_id bigint,
    price bigint,
    quantity bigint,
    -- Expected balances after this trade
    buyer_btc_balance_after bigint,
    buyer_usdt_balance_after bigint,
    seller_btc_balance_after bigint,
    seller_usdt_balance_after bigint
);

// Use UNLOGGED batch
let mut batch = BatchStatement::new(BatchType::Unlogged);

// 1. Insert trade with expected balances
batch.append("INSERT INTO settled_trades_btc_usdt (
    trade_id, ...,
    buyer_btc_balance_after, buyer_usdt_balance_after,
    seller_btc_balance_after, seller_usdt_balance_after
) VALUES (?, ..., ?, ?, ?, ?)",
    (trade.trade_id, ...,
     buyer_btc_after, buyer_usdt_after,
     seller_btc_after, seller_usdt_after)
);

// 2. Update balances (idempotent - can retry)
batch.append("UPDATE user_balances SET available = ? WHERE user_id = ? AND asset_id = ?",
             (buyer_btc_after, buyer_user_id, BTC));
// ... (3 more balance updates)

session.batch(&batch, ()).await?;
```

**Recovery**:
```rust
// If batch partially failed, rebuild balances from trades
pub async fn rebuild_balance_from_trades(
    db: &SettlementDb,
    user_id: u64,
    asset_id: u32,
) -> Result<()> {
    // Get latest trade for this user
    let latest_trade = db.get_latest_trade_for_user(user_id).await?;

    // Use expected balance from trade record
    let expected_balance = if latest_trade.buyer_user_id == user_id && asset_id == BTC {
        latest_trade.buyer_btc_balance_after
    } else if latest_trade.buyer_user_id == user_id && asset_id == USDT {
        latest_trade.buyer_usdt_balance_after
    } // ... (handle seller cases)

    // Force update to expected balance
    db.force_update_balance(user_id, asset_id, expected_balance).await?;

    Ok(())
}
```

**Performance**:
- ✅ **Fast** (UNLOGGED batch)
- ✅ **Recoverable** (expected balances in trade records)
- ⚠️ **Manual recovery** needed

---

## Performance Comparison

| Approach | Throughput | Atomicity | Recovery | Complexity |
|----------|------------|-----------|----------|------------|
| **LOGGED batch** | 300-500/sec | ✅ Automatic | ✅ Automatic | Low |
| **LOGGED micro-batch** | 1000+/sec | ✅ Automatic | ✅ Automatic | Low |
| **UNLOGGED + WAL** | 800/sec | ⚠️ App-level | ⚠️ Manual | High |
| **UNLOGGED + idempotent** | 800/sec | ⚠️ App-level | ⚠️ Manual | High |

---

## Recommendation

### For Settlement Service

**Use LOGGED batch with micro-batching**:

```rust
// Best balance of performance and safety
async fn settle_trades_batch(trades: &[MatchExecData]) -> Result<()> {
    let mut batch = BatchStatement::new(BatchType::Logged);

    for trade in trades {
        batch.append(insert_trade_stmt(trade));
        batch.append(update_balance_stmt(/* ... */));
        // ... (4 balance updates per trade)
    }

    // One LOGGED batch for 10-50 trades
    session.batch(&batch, ()).await?;

    Ok(())
}
```

**Why**:
- ✅ **Good performance** (1000+ trades/sec with batching)
- ✅ **Automatic atomicity** (no manual recovery)
- ✅ **Simple** (no WAL needed)
- ✅ **Safe** (guaranteed consistency)

### Only Use UNLOGGED If:

1. ✅ You need **> 2000 trades/sec** (rare for settlement)
2. ✅ You can implement **application WAL** correctly
3. ✅ You can handle **manual recovery** complexity

**For 99% of cases**: LOGGED batch with micro-batching is **sufficient and safer**.

---

## Summary

**UNLOGGED batch**:
- ✅ **Fast** (~3x faster than LOGGED)
- ❌ **No atomicity** across crashes
- ❌ **Can have partial state**
- ⚠️ **Only safe with application-level recovery**

**LOGGED batch**:
- ⚠️ **Slower** (but acceptable with micro-batching)
- ✅ **Automatic atomicity**
- ✅ **No partial state**
- ✅ **Simpler** (no manual recovery)

**Recommendation**: **Use LOGGED batch** unless you have **extreme performance requirements** (> 2000 trades/sec).
