# Materialized Views in ScyllaDB: How They Work

## What is a Materialized View?

A **Materialized View (MV)** in ScyllaDB is a **database-managed** table that automatically stays in sync with a base table.

## Important Clarification

In my previous explanation, I used "materialized view" loosely to mean **"a table optimized for reads"**. But there are actually **two different approaches**:

### Approach 1: ScyllaDB Native Materialized View (Automatic)

```sql
-- Base table
CREATE TABLE settled_trades (
    trade_id bigint PRIMARY KEY,
    buyer_user_id bigint,
    seller_user_id bigint,
    base_asset int,
    quote_asset int,
    quantity bigint,
    price bigint
);

-- Materialized View (automatically updated by ScyllaDB)
CREATE MATERIALIZED VIEW trades_by_buyer AS
    SELECT * FROM settled_trades
    WHERE buyer_user_id IS NOT NULL AND trade_id IS NOT NULL
    PRIMARY KEY (buyer_user_id, trade_id);
```

**How it works**:
1. You INSERT into `settled_trades`
2. ScyllaDB **automatically** inserts into `trades_by_buyer`
3. Both operations happen **atomically** (same write path)

**Pros**:
- ✅ Automatic synchronization
- ✅ Atomic with base table write
- ✅ No application code needed

**Cons**:
- ❌ **Cannot aggregate** (no SUM, COUNT, etc.)
- ❌ **Cannot join** multiple tables
- ❌ Limited to simple projections and filtering
- ❌ **Performance overhead** on writes

### Approach 2: Application-Managed "Materialized View" (Manual)

This is what I actually recommended - **manually maintaining** a separate table:

```sql
-- Base table (per-symbol trades)
CREATE TABLE settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
    buyer_user_id bigint,
    seller_user_id bigint,
    quantity bigint,
    price bigint
);

-- "Materialized view" (manually maintained)
CREATE TABLE user_balances (
    user_id bigint,
    asset_id int,
    available bigint,
    PRIMARY KEY (user_id, asset_id)
);
```

**How it works**:
```rust
// Application code does BOTH operations in a BATCH
let mut batch = BatchStatement::new(BatchType::Logged);

// 1. Insert trade
batch.append("INSERT INTO settled_trades_btc_usdt ...");

// 2. Update balance (manual "materialization")
batch.append("UPDATE user_balances SET available = available + ? ...");

// Execute atomically
session.batch(&batch, ()).await?;
```

**Pros**:
- ✅ **Full control** over what gets materialized
- ✅ Can do **aggregations** (SUM balances)
- ✅ Can **transform** data (calculate deltas)
- ✅ **Atomic** via BATCH statement
- ✅ Better **performance** (optimized for your use case)

**Cons**:
- ⚠️ More application code
- ⚠️ Must ensure BATCH is used correctly

---

## Why Native Materialized Views DON'T Work for Balances

### Problem: MVs Can't Aggregate

```sql
-- ❌ THIS DOESN'T WORK - MVs can't do SUM
CREATE MATERIALIZED VIEW user_balances AS
    SELECT user_id, asset_id, SUM(quantity) as balance  -- ❌ ERROR!
    FROM settled_trades
    WHERE user_id IS NOT NULL
    GROUP BY user_id, asset_id;
```

ScyllaDB MVs are **denormalization**, not **aggregation**:
- ✅ Can: Re-index same data with different primary key
- ❌ Can't: Calculate sums, counts, or derived values

### What MVs CAN Do

```sql
-- ✅ This works - just re-indexing
CREATE MATERIALIZED VIEW trades_by_user AS
    SELECT trade_id, user_id, quantity, price
    FROM settled_trades
    WHERE user_id IS NOT NULL AND trade_id IS NOT NULL
    PRIMARY KEY (user_id, trade_id);
```

Now you can query:
```sql
-- Fast: Query by user_id
SELECT * FROM trades_by_user WHERE user_id = 1001;
```

But you still need to **calculate balance in application**:
```rust
let trades = db.query("SELECT * FROM trades_by_user WHERE user_id = ?", user_id).await?;
let balance = trades.iter().map(|t| t.quantity).sum();  // ❌ Slow!
```

---

## Recommended Approach: Application-Managed Balance Table

### Schema

```sql
-- Source of truth: Per-symbol trades
CREATE TABLE settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
    output_sequence bigint,
    buyer_user_id bigint,
    seller_user_id bigint,
    price bigint,
    quantity bigint,
    settled_at bigint
);

-- Manually maintained balance table (NOT a ScyllaDB MV)
CREATE TABLE user_balances (
    user_id bigint,
    asset_id int,
    available bigint,
    frozen bigint,
    version bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, asset_id)
);
```

### Implementation

```rust
pub async fn settle_trade_atomically(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    let quote_amount = trade.price * trade.quantity;

    // Create atomic batch
    let mut batch = BatchStatement::new(BatchType::Logged);

    // 1. Insert trade into symbol-specific table
    let trade_table = format!("settled_trades_{}", symbol.to_lowercase());
    batch.append_statement(self.prepare_trade_insert(&trade_table, trade));

    // 2. Update buyer BTC balance (+quantity)
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (trade.quantity as i64, now(), trade.buyer_user_id, trade.base_asset)
    });

    // 3. Update buyer USDT balance (-quote_amount)
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available - ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (quote_amount as i64, now(), trade.buyer_user_id, trade.quote_asset)
    });

    // 4. Update seller BTC balance (-quantity)
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available - ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (trade.quantity as i64, now(), trade.seller_user_id, trade.base_asset)
    });

    // 5. Update seller USDT balance (+quote_amount)
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (quote_amount as i64, now(), trade.seller_user_id, trade.quote_asset)
    });

    // Execute all 5 operations atomically
    self.session.batch(&batch, ()).await?;

    Ok(())
}
```

### Why This Works

1. **BATCH Statement Guarantees**:
   - All operations write to the **same partition** (same timestamp)
   - ScyllaDB applies them **atomically**
   - Either all succeed or all fail

2. **Idempotency**:
   - Trade ID is primary key → duplicate inserts fail
   - If batch is retried, trade insert fails → balance updates don't happen
   - Natural idempotency

3. **Fast Reads**:
   ```rust
   // O(1) query - single partition read
   let balances = db.query(
       "SELECT asset_id, available FROM user_balances WHERE user_id = ?",
       user_id
   ).await?;
   ```

---

## Comparison

| Approach | Aggregation | Atomicity | Performance | Complexity |
|----------|-------------|-----------|-------------|------------|
| **Native MV** | ❌ No | ✅ Yes | ⚠️ Slower writes | Low |
| **Manual Table + BATCH** | ✅ Yes | ✅ Yes | ✅ Fast reads | Medium |
| **Event Sourcing Only** | ✅ Yes | ✅ Yes | ❌ Slow reads | Low |

---

## When to Use Each Approach

### Use Native Materialized View When:
- ✅ You just need to **re-index** data with different primary key
- ✅ No aggregation needed
- ✅ Example: Query trades by `buyer_user_id` instead of `trade_id`

### Use Manual Table + BATCH When:
- ✅ You need **aggregations** (SUM, COUNT, etc.)
- ✅ You need **derived values** (balances, totals)
- ✅ You want **fast reads** with pre-computed values
- ✅ **This is our use case!**

### Use Event Sourcing Only When:
- ✅ Audit trail is more important than read performance
- ✅ Reads are infrequent
- ✅ You can afford to scan/aggregate on every read

---

## Summary

**"Materialized View" in my explanation = Application-managed balance table**

- **NOT** a ScyllaDB native Materialized View
- **IS** a manually maintained table updated via BATCH
- **Provides** atomicity through BATCH statement
- **Enables** fast balance queries (O(1) instead of O(n))
- **Maintains** audit trail in per-symbol trade tables

This gives you:
1. ✅ **Atomicity** (BATCH ensures all-or-nothing)
2. ✅ **Fast reads** (pre-aggregated balances)
3. ✅ **Audit trail** (complete trade history)
4. ✅ **Idempotency** (trade ID deduplication)

**Recommended**: Use **Application-Managed Balance Table** with BATCH statements.
