# Hybrid Approach: Symbol-Partitioned Trades + Materialized Balance Table

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Source of Truth: Per-Symbol Trade Tables                   │
│  - settled_trades_btc_usdt                                  │
│  - settled_trades_eth_usdt                                  │
│  - settled_trades_btc_eth                                   │
└─────────────────────────────────────────────────────────────┘
                        │
                        │ (Atomic Update)
                        ▼
┌─────────────────────────────────────────────────────────────┐
│  Materialized View: user_balances (for fast reads)         │
│  PRIMARY KEY: (user_id, asset_id)                          │
└─────────────────────────────────────────────────────────────┘
```

## Solution: Atomic Batch Insert

### Schema

```sql
-- Per-symbol trade tables (source of truth)
CREATE TABLE settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
    output_sequence bigint,
    buyer_user_id bigint,
    seller_user_id bigint,
    price bigint,
    quantity bigint,
    settled_at bigint,
    -- Include balance deltas for atomic update
    buyer_btc_delta bigint,
    buyer_usdt_delta bigint,
    seller_btc_delta bigint,
    seller_usdt_delta bigint
);

-- Materialized balance table (for fast reads)
CREATE TABLE user_balances (
    user_id bigint,
    asset_id int,
    available bigint,
    frozen bigint,
    version bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, asset_id)
);

-- Index for fast user balance queries
CREATE INDEX ON user_balances (user_id);
```

### Key Insight: Single BATCH Statement

```rust
pub async fn settle_trade_atomically(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    let quote_amount = trade.price * trade.quantity;

    // Calculate deltas
    let buyer_base_delta = trade.quantity as i64;
    let buyer_quote_delta = -(quote_amount as i64);
    let seller_base_delta = -(trade.quantity as i64);
    let seller_quote_delta = quote_amount as i64;

    // Create BATCH statement
    let mut batch = BatchStatement::new(BatchType::Logged);

    // 1. Insert trade into symbol-specific table
    let trade_table = format!("settled_trades_{}", symbol.to_lowercase());
    batch.append_statement(PreparedStatement {
        query: format!(
            "INSERT INTO {} (trade_id, output_sequence, buyer_user_id, seller_user_id,
                            price, quantity, settled_at,
                            buyer_btc_delta, buyer_usdt_delta,
                            seller_btc_delta, seller_usdt_delta)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            trade_table
        ),
        values: (
            trade.trade_id,
            trade.output_sequence,
            trade.buyer_user_id,
            trade.seller_user_id,
            trade.price,
            trade.quantity,
            trade.settled_at,
            buyer_base_delta,
            buyer_quote_delta,
            seller_base_delta,
            seller_quote_delta,
        )
    });

    // 2. Update buyer's base asset balance
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (buyer_base_delta, now(), trade.buyer_user_id, trade.base_asset)
    });

    // 3. Update buyer's quote asset balance
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (buyer_quote_delta, now(), trade.buyer_user_id, trade.quote_asset)
    });

    // 4. Update seller's base asset balance
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (seller_base_delta, now(), trade.seller_user_id, trade.base_asset)
    });

    // 5. Update seller's quote asset balance
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = version + 1, updated_at = ?
                WHERE user_id = ? AND asset_id = ?",
        values: (seller_quote_delta, now(), trade.seller_user_id, trade.quote_asset)
    });

    // Execute all 5 operations atomically
    self.session.batch(&batch, ()).await?;

    Ok(())
}
```

## Benefits

### ✅ 1. **Atomic Consistency**
- **Single BATCH** = All 5 operations succeed or all fail
- Trade is inserted **only if** all balance updates succeed
- **Impossible** to have trade without balance update

### ✅ 2. **Fast Balance Reads**
```rust
// O(1) query - single partition read
pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<Balance>> {
    let query = "SELECT asset_id, available, frozen, version
                 FROM user_balances
                 WHERE user_id = ?";

    self.session.query(query, (user_id,)).await
}
```

### ✅ 3. **Audit Trail**
- Per-symbol trade tables have complete history
- Can rebuild balances from trades if needed
- Balance deltas stored in trade records

### ✅ 4. **Idempotency**
```rust
// Trade ID is primary key - duplicate inserts fail
// Balance updates use += so they're naturally idempotent IF trade insert succeeds
```

## Idempotency Strategy

### Problem: What if batch is retried?

**Solution: Use trade_id as deduplication key**

```rust
pub async fn settle_trade_atomically(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    // 1. Check if trade already exists
    let exists = self.trade_exists(symbol, trade.trade_id).await?;
    if exists {
        log::debug!("Trade {} already settled, skipping", trade.trade_id);
        return Ok(());
    }

    // 2. Execute batch (trade insert + balance updates)
    let mut batch = BatchStatement::new(BatchType::Logged);
    // ... (as above)

    match self.session.batch(&batch, ()).await {
        Ok(_) => Ok(()),
        Err(e) if is_duplicate_key_error(&e) => {
            // Trade was inserted by another process, balances already updated
            log::debug!("Trade {} already settled (concurrent insert)", trade.trade_id);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
```

## Alternative: Optimistic Locking with Versions

If you want **stronger** idempotency guarantees:

```rust
pub async fn settle_trade_with_version_check(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<bool> {
    let quote_amount = trade.price * trade.quantity;

    // Use LWT for balance updates (slower but safer)
    let mut batch = BatchStatement::new(BatchType::Logged);

    // 1. Insert trade (will fail if duplicate)
    batch.append(insert_trade_stmt(...));

    // 2. Update balances with version check
    batch.append_statement(PreparedStatement {
        query: "UPDATE user_balances
                SET available = available + ?, version = ?
                WHERE user_id = ? AND asset_id = ?
                IF version = ?",  // <-- LWT condition
        values: (
            buyer_base_delta,
            trade.buyer_base_version + 1,
            trade.buyer_user_id,
            trade.base_asset,
            trade.buyer_base_version  // Expected version
        )
    });

    // ... (repeat for other 3 balances)

    let result = self.session.batch(&batch, ()).await?;

    // Check if all LWT conditions passed
    Ok(check_all_lwt_applied(result))
}
```

**Trade-off**:
- ✅ Stronger idempotency (version-based)
- ❌ Slower (LWT has performance cost)
- ❌ More complex error handling

## Recommended Approach

### For Production: **Simple Batch (No LWT)**

```rust
// Pseudo-code
async fn settle_trade(symbol: &str, trade: &MatchExecData) -> Result<()> {
    // 1. Check if already settled
    if trade_exists(symbol, trade.trade_id).await? {
        return Ok(()); // Idempotent
    }

    // 2. Execute atomic batch
    let batch = create_settlement_batch(symbol, trade);
    session.batch(&batch, ()).await?;

    Ok(())
}
```

**Why this works**:
1. Trade ID is primary key → duplicate inserts fail
2. Pre-check prevents unnecessary batch execution
3. Balance updates use `+=` → naturally idempotent within a batch
4. BATCH ensures atomicity

## Balance Rebuild Function (Safety Net)

```rust
pub async fn rebuild_user_balance(
    &self,
    user_id: u64,
    asset_id: u32,
) -> Result<()> {
    let mut total_delta = 0i64;

    // Get all symbols
    let symbols = vec!["btc_usdt", "eth_usdt", "btc_eth"];

    for symbol in symbols {
        let table = format!("settled_trades_{}", symbol);

        // Get all trades affecting this user and asset
        let query = format!(
            "SELECT buyer_user_id, seller_user_id,
                    buyer_btc_delta, buyer_usdt_delta,
                    seller_btc_delta, seller_usdt_delta
             FROM {}
             WHERE buyer_user_id = ? OR seller_user_id = ?",
            table
        );

        let trades = self.session.query(query, (user_id, user_id)).await?;

        for trade in trades.rows? {
            // Accumulate deltas
            if trade.buyer_user_id == user_id {
                if asset_id == BTC {
                    total_delta += trade.buyer_btc_delta;
                } else {
                    total_delta += trade.buyer_usdt_delta;
                }
            } else {
                if asset_id == BTC {
                    total_delta += trade.seller_btc_delta;
                } else {
                    total_delta += trade.seller_usdt_delta;
                }
            }
        }
    }

    // Force update balance
    self.session.query(
        "UPDATE user_balances SET available = ?, version = version + 1
         WHERE user_id = ? AND asset_id = ?",
        (total_delta, user_id, asset_id)
    ).await?;

    Ok(())
}
```

## Summary

| Aspect | Solution |
|--------|----------|
| **Trade Storage** | Per-symbol tables (source of truth) |
| **Balance Storage** | Single `user_balances` table (materialized view) |
| **Atomicity** | BATCH statement (1 trade insert + 4 balance updates) |
| **Fast Reads** | ✅ Single query to `user_balances` by `user_id` |
| **Idempotency** | Trade ID primary key + pre-check |
| **Recovery** | Rebuild from per-symbol trade tables |
| **Audit Trail** | ✅ Complete history in trade tables |

## Next Steps

1. **Create schema** with per-symbol trade tables + user_balances
2. **Implement** `settle_trade_atomically()` with BATCH
3. **Add** `rebuild_user_balance()` as safety net
4. **Test** with concurrent settlement scenarios
5. **Monitor** batch execution latency

This gives you **both atomicity AND fast reads**! 🎯
