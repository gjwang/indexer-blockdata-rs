# Symbol-Partitioned Trade Storage

## Concept

Instead of one `settled_trades` table for all symbols, create **separate tables per trading pair**:

```
settled_trades_btc_usdt
settled_trades_eth_usdt
settled_trades_btc_eth
```

## Why This Solves Atomicity

### Current Problem
```rust
// Single transaction across multiple tables
1. INSERT INTO settled_trades (trade_id, ...) VALUES (...)
2. UPDATE user_balances SET available = ... WHERE user_id = buyer AND asset_id = USDT
3. UPDATE user_balances SET available = ... WHERE user_id = buyer AND asset_id = BTC
4. UPDATE user_balances SET available = ... WHERE user_id = seller AND asset_id = BTC
5. UPDATE user_balances SET available = ... WHERE user_id = seller AND asset_id = USDT
```
❌ **5 separate operations** - can partially fail

### With Symbol-Partitioned Storage

Each symbol has its own isolated storage:

```sql
-- BTC/USDT trades and balances
CREATE TABLE settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
    buyer_user_id bigint,
    seller_user_id bigint,
    price bigint,
    quantity bigint,
    buyer_btc_balance bigint,    -- Snapshot of buyer's BTC balance after trade
    buyer_usdt_balance bigint,   -- Snapshot of buyer's USDT balance after trade
    seller_btc_balance bigint,   -- Snapshot of seller's BTC balance after trade
    seller_usdt_balance bigint,  -- Snapshot of seller's USDT balance after trade
    settled_at bigint
);

-- User balances are derived by scanning trades
-- No separate user_balances table needed!
```

## Key Insight: Event Sourcing

**Trades are the source of truth, balances are derived**

```rust
// Get user's current BTC balance in BTC/USDT pair
pub async fn get_user_btc_balance_in_btc_usdt(user_id: u64) -> i64 {
    let trades = query("SELECT * FROM settled_trades_btc_usdt
                        WHERE buyer_user_id = ? OR seller_user_id = ?",
                        user_id, user_id);

    let mut balance = 0;
    for trade in trades {
        if trade.buyer_user_id == user_id {
            balance += trade.quantity;  // Bought BTC
        } else {
            balance -= trade.quantity;  // Sold BTC
        }
    }
    balance
}
```

## Advantages

### ✅ 1. **Perfect Atomicity**
- Each trade insert is a **single operation**
- No separate balance updates needed
- **Impossible to have inconsistent state**

### ✅ 2. **Complete Audit Trail**
- Every trade includes balance snapshots
- Can reconstruct balance at any point in time
- Perfect for compliance/auditing

### ✅ 3. **No Version Conflicts**
- No LWT needed
- No version-based idempotency complexity
- Trade ID is the only deduplication key

### ✅ 4. **Symbol Isolation**
- BTC/USDT trades completely independent from ETH/USDT
- Failure in one symbol doesn't affect others
- Easy to add new trading pairs

### ✅ 5. **Simplified Recovery**
- Just replay trades in order
- Balances automatically correct
- No manual reconciliation needed

## Implementation

### Schema Design

```sql
-- Per-symbol trade table
CREATE TABLE settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
    output_sequence bigint,
    match_seq bigint,
    buy_order_id bigint,
    sell_order_id bigint,
    buyer_user_id bigint,
    seller_user_id bigint,
    price bigint,
    quantity bigint,
    buyer_refund bigint,
    seller_refund bigint,
    settled_at bigint,

    -- Balance snapshots (optional but useful)
    buyer_btc_balance_after bigint,
    buyer_usdt_balance_after bigint,
    seller_btc_balance_after bigint,
    seller_usdt_balance_after bigint
);

-- Index for user queries
CREATE INDEX ON settled_trades_btc_usdt (buyer_user_id);
CREATE INDEX ON settled_trades_btc_usdt (seller_user_id);
CREATE INDEX ON settled_trades_btc_usdt (output_sequence);
```

### Rust Implementation

```rust
impl SettlementDb {
    pub async fn insert_trade_with_balances(
        &self,
        symbol: &str,
        trade: &MatchExecData,
        buyer_btc_balance: i64,
        buyer_usdt_balance: i64,
        seller_btc_balance: i64,
        seller_usdt_balance: i64,
    ) -> Result<()> {
        let table_name = format!("settled_trades_{}", symbol.to_lowercase());

        let query = format!(
            "INSERT INTO {} (
                trade_id, output_sequence, match_seq,
                buy_order_id, sell_order_id,
                buyer_user_id, seller_user_id,
                price, quantity,
                buyer_refund, seller_refund,
                settled_at,
                buyer_btc_balance_after,
                buyer_usdt_balance_after,
                seller_btc_balance_after,
                seller_usdt_balance_after
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            table_name
        );

        self.session.query(query, (
            trade.trade_id,
            trade.output_sequence,
            trade.match_seq,
            trade.buy_order_id,
            trade.sell_order_id,
            trade.buyer_user_id,
            trade.seller_user_id,
            trade.price,
            trade.quantity,
            trade.buyer_refund,
            trade.seller_refund,
            trade.settled_at,
            buyer_btc_balance,
            buyer_usdt_balance,
            seller_btc_balance,
            seller_usdt_balance,
        )).await?;

        Ok(())
    }

    pub async fn get_user_balance(
        &self,
        symbol: &str,
        user_id: u64,
        asset: &str, // "BTC" or "USDT"
    ) -> Result<i64> {
        let table_name = format!("settled_trades_{}", symbol.to_lowercase());

        // Get latest trade for this user
        let query = format!(
            "SELECT buyer_user_id, seller_user_id,
                    buyer_btc_balance_after, buyer_usdt_balance_after,
                    seller_btc_balance_after, seller_usdt_balance_after
             FROM {}
             WHERE buyer_user_id = ? OR seller_user_id = ?
             ORDER BY output_sequence DESC
             LIMIT 1",
            table_name
        );

        let result = self.session.query(query, (user_id, user_id)).await?;

        if let Some(row) = result.rows?.into_iter().next() {
            let (buyer_id, seller_id, buyer_btc, buyer_usdt, seller_btc, seller_usdt):
                (i64, i64, i64, i64, i64, i64) = row.into_typed()?;

            if buyer_id == user_id as i64 {
                return Ok(if asset == "BTC" { buyer_btc } else { buyer_usdt });
            } else {
                return Ok(if asset == "BTC" { seller_btc } else { seller_usdt });
            }
        }

        Ok(0) // No trades yet
    }
}
```

### Settlement Service

```rust
async fn settle_trade(
    db: &SettlementDb,
    trade: &MatchExecData,
    symbol: &str,
) -> Result<()> {
    // 1. Calculate new balances
    let quote_amount = trade.price * trade.quantity;

    // Get current balances (from last trade)
    let buyer_btc_before = db.get_user_balance(symbol, trade.buyer_user_id, "BTC").await?;
    let buyer_usdt_before = db.get_user_balance(symbol, trade.buyer_user_id, "USDT").await?;
    let seller_btc_before = db.get_user_balance(symbol, trade.seller_user_id, "BTC").await?;
    let seller_usdt_before = db.get_user_balance(symbol, trade.seller_user_id, "USDT").await?;

    // Calculate new balances
    let buyer_btc_after = buyer_btc_before + trade.quantity as i64;
    let buyer_usdt_after = buyer_usdt_before - quote_amount as i64;
    let seller_btc_after = seller_btc_before - trade.quantity as i64;
    let seller_usdt_after = seller_usdt_before + quote_amount as i64;

    // 2. Insert trade with balance snapshots (ATOMIC!)
    db.insert_trade_with_balances(
        symbol,
        trade,
        buyer_btc_after,
        buyer_usdt_after,
        seller_btc_after,
        seller_usdt_after,
    ).await?;

    Ok(())
}
```

## Trade-offs

### Pros
- ✅ **Perfect atomicity** - single INSERT
- ✅ **No balance table** - trades are source of truth
- ✅ **Complete audit trail** - balance at every trade
- ✅ **Symbol isolation** - failures don't spread
- ✅ **Simple recovery** - just replay trades
- ✅ **No version conflicts** - no LWT needed

### Cons
- ⚠️ **More tables** - one per symbol (but manageable)
- ⚠️ **Balance queries slower** - need to scan trades (can optimize with materialized views)
- ⚠️ **Cross-symbol balance** - need to query multiple tables
- ⚠️ **Schema migration** - adding symbols requires new tables

## Optimization: Materialized View for Balances

```sql
-- Materialized view for fast balance lookups
CREATE MATERIALIZED VIEW user_balances_btc_usdt AS
    SELECT user_id, asset, balance
    FROM (
        SELECT buyer_user_id AS user_id, 'BTC' AS asset, buyer_btc_balance_after AS balance
        FROM settled_trades_btc_usdt
        UNION ALL
        SELECT seller_user_id AS user_id, 'BTC' AS asset, seller_btc_balance_after AS balance
        FROM settled_trades_btc_usdt
    )
    PRIMARY KEY (user_id, asset);
```

## Recommendation

**This is the best approach for your use case!**

1. ✅ **Solves atomicity** - single INSERT per trade
2. ✅ **Maintains consistency** - balances derived from trades
3. ✅ **Scales well** - symbol isolation
4. ✅ **Simple to implement** - less code than LWT batching
5. ✅ **Audit-friendly** - complete history

**Next Steps**:
1. Create per-symbol trade tables
2. Update settlement service to use symbol-specific tables
3. Add materialized views for fast balance queries
4. Migrate existing data (if any)
