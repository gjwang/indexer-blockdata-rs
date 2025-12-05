# Efficient User Balance Queries

## The Goal

**Query**: Get all asset balances for a user in one fast operation

```rust
// User 1001 has:
// BTC: 1.5
// USDT: 10000
// ETH: 5.2
let balances = db.get_user_all_balances(1001).await?;
```

---

## Schema Design for Efficient Queries

### Option 1: Partition by User (RECOMMENDED)

```sql
CREATE TABLE user_balances (
    user_id bigint,        -- Partition key
    asset_id int,          -- Clustering key
    available bigint,
    frozen bigint,
    version bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, asset_id)
);
```

**Why this works**:
- **Partition key = user_id** → All user's assets in **one partition**
- **Clustering key = asset_id** → Assets sorted within partition
- **Single partition read** → O(1) query, very fast

### Query

```rust
pub async fn get_user_all_balances(
    &self,
    user_id: u64,
) -> Result<Vec<UserBalance>> {
    let query = "
        SELECT asset_id, available, frozen, version, updated_at
        FROM user_balances
        WHERE user_id = ?
    ";

    let result = self.session.query(query, (user_id as i64,)).await?;

    let mut balances = Vec::new();
    if let Some(rows) = result.rows {
        for row in rows {
            let (asset_id, available, frozen, version, updated_at):
                (i32, i64, i64, i64, i64) = row.into_typed()?;

            balances.push(UserBalance {
                user_id,
                asset_id: asset_id as u32,
                available: available as u64,
                frozen: frozen as u64,
                version: version as u64,
                updated_at: updated_at as u64,
            });
        }
    }

    Ok(balances)
}
```

**Performance**:
- ✅ **O(1) query** - Single partition read
- ✅ **~1-2ms latency** - Very fast
- ✅ **Scales well** - Each user's data is isolated

**Example result**:
```json
[
  {"asset_id": 1, "available": 150000000, "frozen": 0},  // BTC (1.5 BTC in satoshis)
  {"asset_id": 2, "available": 1000000000, "frozen": 0}, // USDT (10000 USDT)
  {"asset_id": 3, "available": 520000000, "frozen": 0}   // ETH (5.2 ETH)
]
```

---

### Option 2: Partition by Asset (WRONG for this use case)

```sql
-- ❌ DON'T DO THIS for "get all user balances"
CREATE TABLE user_balances (
    asset_id int,          -- Partition key
    user_id bigint,        -- Clustering key
    available bigint,
    PRIMARY KEY (asset_id, user_id)
);
```

**Why this is bad**:
- To get all balances for user 1001, you need to query **each asset separately**:
  ```sql
  SELECT * FROM user_balances WHERE asset_id = 1 AND user_id = 1001;  -- BTC
  SELECT * FROM user_balances WHERE asset_id = 2 AND user_id = 1001;  -- USDT
  SELECT * FROM user_balances WHERE asset_id = 3 AND user_id = 1001;  -- ETH
  ```
- ❌ **Multiple queries** (one per asset)
- ❌ **Slow** (N queries for N assets)

**Only use this if**:
- You need to query "all users holding BTC"
- But this is **not** your use case

---

## How Updates Work with Partition by User

### Update Single Balance

```rust
pub async fn update_user_balance(
    &self,
    user_id: u64,
    asset_id: u32,
    delta: i64,
) -> Result<()> {
    let query = "
        UPDATE user_balances
        SET available = available + ?,
            version = version + 1,
            updated_at = ?
        WHERE user_id = ? AND asset_id = ?
    ";

    self.session.query(
        query,
        (delta, now(), user_id as i64, asset_id as i32)
    ).await?;

    Ok(())
}
```

**Performance**:
- ✅ **O(1) update** - Direct partition + clustering key lookup
- ✅ **~1-2ms latency**

### Update Multiple Balances (Trade Settlement)

```rust
pub async fn settle_trade_atomically(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    let quote_amount = trade.price * trade.quantity;

    let mut batch = BatchStatement::new(BatchType::Logged);

    // 1. Insert trade
    batch.append(insert_trade_stmt(symbol, trade));

    // 2. Update buyer BTC (user_id=buyer, asset_id=BTC)
    batch.append("UPDATE user_balances
                  SET available = available + ?, version = version + 1, updated_at = ?
                  WHERE user_id = ? AND asset_id = ?",
                 (trade.quantity as i64, now(), trade.buyer_user_id, BTC));

    // 3. Update buyer USDT (user_id=buyer, asset_id=USDT)
    batch.append("UPDATE user_balances
                  SET available = available - ?, version = version + 1, updated_at = ?
                  WHERE user_id = ? AND asset_id = ?",
                 (quote_amount as i64, now(), trade.buyer_user_id, USDT));

    // 4. Update seller BTC (user_id=seller, asset_id=BTC)
    batch.append("UPDATE user_balances
                  SET available = available - ?, version = version + 1, updated_at = ?
                  WHERE user_id = ? AND asset_id = ?",
                 (trade.quantity as i64, now(), trade.seller_user_id, BTC));

    // 5. Update seller USDT (user_id=seller, asset_id=USDT)
    batch.append("UPDATE user_balances
                  SET available = available + ?, version = version + 1, updated_at = ?
                  WHERE user_id = ? AND asset_id = ?",
                 (quote_amount as i64, now(), trade.seller_user_id, USDT));

    // Execute atomically
    self.session.batch(&batch, ()).await?;

    Ok(())
}
```

**Performance**:
- ✅ **4 partition updates** (2 users × 2 assets)
- ✅ **Atomic** (LOGGED batch)
- ✅ **~5-10ms latency**

---

## Partition Distribution

### How Data is Distributed

```
ScyllaDB Cluster:
┌─────────────────────────────────────────────────────────┐
│ Node 1                                                   │
│  - user_id: 1001 → [BTC, USDT, ETH]                    │
│  - user_id: 1005 → [BTC, USDT]                         │
├─────────────────────────────────────────────────────────┤
│ Node 2                                                   │
│  - user_id: 2001 → [BTC, USDT, ETH, DOGE]              │
│  - user_id: 2010 → [BTC]                                │
├─────────────────────────────────────────────────────────┤
│ Node 3                                                   │
│  - user_id: 3001 → [BTC, USDT]                         │
│  - user_id: 3050 → [BTC, USDT, ETH]                    │
└─────────────────────────────────────────────────────────┘
```

**Key points**:
- Each **user_id** is a partition
- All **assets for one user** are in the **same partition**
- Partitions are **distributed** across nodes by hash(user_id)

### Why This is Efficient

**Query: Get all balances for user 1001**
```
1. Hash(1001) → Node 1
2. Read partition 1001 from Node 1
3. Return all assets in that partition
```

- ✅ **Single node** access
- ✅ **Single partition** read
- ✅ **No scatter-gather** across nodes

---

## Complete Example

### Schema

```sql
CREATE TABLE user_balances (
    user_id bigint,
    asset_id int,
    available bigint,
    frozen bigint,
    version bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, asset_id)
) WITH CLUSTERING ORDER BY (asset_id ASC);
```

### Rust Implementation

```rust
#[derive(Debug, Clone)]
pub struct UserBalance {
    pub user_id: u64,
    pub asset_id: u32,
    pub available: u64,
    pub frozen: u64,
    pub version: u64,
    pub updated_at: u64,
}

impl SettlementDb {
    /// Get all asset balances for a user (O(1) query)
    pub async fn get_user_all_balances(
        &self,
        user_id: u64,
    ) -> Result<Vec<UserBalance>> {
        const QUERY: &str = "
            SELECT asset_id, available, frozen, version, updated_at
            FROM user_balances
            WHERE user_id = ?
        ";

        let result = self.session.query(QUERY, (user_id as i64,)).await?;

        let mut balances = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                let (asset_id, available, frozen, version, updated_at):
                    (i32, i64, i64, i64, i64) = row.into_typed()?;

                balances.push(UserBalance {
                    user_id,
                    asset_id: asset_id as u32,
                    available: available as u64,
                    frozen: frozen as u64,
                    version: version as u64,
                    updated_at: updated_at as u64,
                });
            }
        }

        Ok(balances)
    }

    /// Get single asset balance for a user (O(1) query)
    pub async fn get_user_balance(
        &self,
        user_id: u64,
        asset_id: u32,
    ) -> Result<Option<UserBalance>> {
        const QUERY: &str = "
            SELECT available, frozen, version, updated_at
            FROM user_balances
            WHERE user_id = ? AND asset_id = ?
        ";

        let result = self.session.query(
            QUERY,
            (user_id as i64, asset_id as i32)
        ).await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                let (available, frozen, version, updated_at): (i64, i64, i64, i64) =
                    row.into_typed()?;

                return Ok(Some(UserBalance {
                    user_id,
                    asset_id,
                    available: available as u64,
                    frozen: frozen as u64,
                    version: version as u64,
                    updated_at: updated_at as u64,
                }));
            }
        }

        Ok(None)
    }
}
```

### Gateway API Usage

```rust
// In gateway.rs
async fn get_user_balance(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<UserIdParam>,
) -> Result<Json<ApiResponse<Vec<BalanceResponse>>>, (StatusCode, String)> {
    let user_id = state.user_manager.get_user_id(&params.api_key)?;

    // Get all balances in one query (O(1))
    let balances = state.settlement_db.get_user_all_balances(user_id).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Convert to client format
    let response: Vec<BalanceResponse> = balances
        .into_iter()
        .map(|b| BalanceResponse {
            asset: state.symbol_manager.get_asset_name(b.asset_id),
            available: state.balance_manager.to_client_amount(b.asset_id, b.available),
            frozen: state.balance_manager.to_client_amount(b.asset_id, b.frozen),
        })
        .collect();

    Ok(Json(ApiResponse::success(response)))
}
```

---

## Performance Characteristics

| Operation | Complexity | Latency | Notes |
|-----------|------------|---------|-------|
| **Get all balances** | O(1) | 1-2ms | Single partition read |
| **Get single balance** | O(1) | 1-2ms | Partition + clustering key |
| **Update single balance** | O(1) | 1-2ms | Direct update |
| **Update 4 balances (trade)** | O(1) | 5-10ms | BATCH with 4 updates |

---

## Summary

**Schema**: `PRIMARY KEY (user_id, asset_id)`
- ✅ **Partition by user_id** → All user's assets in one partition
- ✅ **Cluster by asset_id** → Assets sorted within partition

**Query**: `SELECT * FROM user_balances WHERE user_id = ?`
- ✅ **O(1) complexity** → Single partition read
- ✅ **1-2ms latency** → Very fast
- ✅ **Returns all assets** → One query gets everything

**This is the optimal schema for your use case!**
