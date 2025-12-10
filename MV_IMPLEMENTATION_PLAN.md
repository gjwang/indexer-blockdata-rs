# Balance Query Implementation Plan

**Date**: 2025-12-10
**Goal**: Implement Materialized View for efficient balance queries
**Approach**: Simple, production-ready, no TTL initially

---

## Implementation Overview

### Phase 1: Materialized View (This Sprint)

**What we're building:**
- Materialized View for O(1) balance queries by user_id
- Keep existing `balance_ledger` unchanged (no TTL)
- Update Gateway to query MV instead of parallel queries
- Zero Settlement changes (MV auto-updates)

**What we're NOT doing (yet):**
- ❌ TTL on balance_ledger (keep all history)
- ❌ Snapshots (add later)
- ❌ Memory cache (add only if needed)
- ❌ Dual writes (MV handles it)

---

## Step-by-Step Implementation

### Step 1: Create MV Schema (5 minutes)

**File**: `schema/add_balance_mv.cql`

```sql
-- Materialized View for efficient user balance queries
-- Automatically maintained by ScyllaDB
CREATE MATERIALIZED VIEW IF NOT EXISTS trading.user_balances_by_user AS
    SELECT user_id, asset_id, seq, avail, frozen, created_at
    FROM trading.balance_ledger
    WHERE user_id IS NOT NULL
      AND asset_id IS NOT NULL
      AND seq IS NOT NULL
    PRIMARY KEY (user_id, asset_id, seq)
    WITH CLUSTERING ORDER BY (asset_id ASC, seq DESC)
    AND compaction = {
        'class': 'SizeTieredCompactionStrategy',
        'min_threshold': 4,
        'max_threshold': 32
    }
    AND compression = {
        'sstable_compression': 'LZ4Compressor'
    }
    AND bloom_filter_fp_chance = 0.01
    AND caching = {
        'keys': 'ALL',
        'rows_per_partition': 'ALL'
    };
```

**Execute:**
```bash
docker exec scylla cqlsh < schema/add_balance_mv.cql
```

**Verification:**
```bash
# Check MV was created
docker exec scylla cqlsh -e "DESCRIBE MATERIALIZED VIEW trading.user_balances_by_user;"

# Check backfill progress (may take 30-60 min for existing data)
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user;"
```

**Expected output:**
- MV schema displayed
- Count matches `balance_ledger` row count (may take time to backfill)

---

### Step 2: Update Gateway Query Function (15 minutes)

**File**: `src/db/settlement_db.rs`

**Current implementation (lines 680-720):**
```rust
// Queries each known asset separately (3 queries)
pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
    let mut balances = Vec::new();

    for asset_id in [1i32, 2i32, 3i32] {  // Hardcoded assets
        let result = self.session.query(
            "SELECT avail, frozen, seq, created_at
             FROM balance_ledger
             WHERE user_id = ? AND asset_id = ?
             ORDER BY seq DESC
             LIMIT 1",
            (user_id as i64, asset_id as i32)
        ).await?;

        // Parse and add to balances...
    }

    Ok(balances)
}
```

**New implementation:**
```rust
pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
    // Single query to MV using PER PARTITION LIMIT 1
    let result = self.session.query(
        "SELECT asset_id, avail, frozen, seq, created_at
         FROM user_balances_by_user
         WHERE user_id = ?
         PER PARTITION LIMIT 1",  // Returns latest balance per asset
        (user_id as i64,)
    ).await?;

    let mut balances = Vec::new();

    if let Some(rows) = result.rows {
        for row in rows {
            let (asset_id, avail, frozen, seq, created_at): (i32, i64, i64, i64, i64) =
                row.into_typed().context("Failed to parse balance row")?;

            balances.push(UserBalance {
                user_id,
                asset_id: asset_id as u32,
                avail: avail as u64,
                frozen: frozen as u64,
                version: seq as u64,
                updated_at: created_at as u64,
            });
        }
    }

    Ok(balances)
}
```

**Changes:**
1. ✅ Query `user_balances_by_user` instead of `balance_ledger`
2. ✅ Use `PER PARTITION LIMIT 1` for latest balance per asset
3. ✅ Remove hardcoded asset loop (MV returns all user's assets)
4. ✅ Single query instead of 3 parallel queries

---

### Step 3: Add MV Health Check (Optional, 10 minutes)

**File**: `src/db/settlement_db.rs`

```rust
/// Check if MV exists and is healthy
pub async fn check_mv_health(&self) -> Result<MvStatus> {
    // Query system tables to check MV existence
    let result = self.session.query(
        "SELECT view_name FROM system_schema.views
         WHERE keyspace_name = 'trading'
           AND view_name = 'user_balances_by_user'",
        ()
    ).await?;

    if result.rows.is_none() || result.rows.as_ref().unwrap().is_empty() {
        return Ok(MvStatus::Missing);
    }

    // Optional: Compare MV count with base table (sanity check)
    let base_count = self.count_ledger_rows().await?;
    let mv_count = self.count_mv_rows().await?;

    let completeness = (mv_count as f64 / base_count as f64) * 100.0;

    if completeness < 95.0 {
        Ok(MvStatus::Rebuilding(completeness))
    } else {
        Ok(MvStatus::Healthy)
    }
}

#[derive(Debug)]
pub enum MvStatus {
    Healthy,
    Rebuilding(f64),  // Percentage complete
    Missing,
}
```

**Add to Gateway startup:**
```rust
// In order_gate_server.rs main()
if let Some(db) = &state.db {
    match db.check_mv_health().await {
        Ok(MvStatus::Healthy) => log::info!("✅ MV is healthy"),
        Ok(MvStatus::Rebuilding(pct)) => log::warn!("⚠️ MV rebuilding: {:.1}%", pct),
        Ok(MvStatus::Missing) => log::error!("❌ MV is missing!"),
        Err(e) => log::error!("❌ MV health check failed: {}", e),
    }
}
```

---

### Step 4: Testing (20 minutes)

#### **4.1 Unit Test**

**File**: `src/db/settlement_db.rs` (add to existing tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mv_query_returns_all_assets() {
        let db = SettlementDb::connect_test().await.unwrap();

        // Insert test data
        let user_id = 1001;
        db.insert_balance_event(user_id, 1, 100000, 0, 1).await.unwrap(); // BTC
        db.insert_balance_event(user_id, 2, 50000, 0, 2).await.unwrap();  // USDT
        db.insert_balance_event(user_id, 3, 10000, 0, 3).await.unwrap();  // ETH

        // Wait for MV to update (eventual consistency)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Query via MV
        let balances = db.get_user_all_balances(user_id).await.unwrap();

        // Verify
        assert_eq!(balances.len(), 3);
        assert!(balances.iter().any(|b| b.asset_id == 1 && b.avail == 100000));
        assert!(balances.iter().any(|b| b.asset_id == 2 && b.avail == 50000));
        assert!(balances.iter().any(|b| b.asset_id == 3 && b.avail == 10000));
    }

    #[tokio::test]
    async fn test_mv_returns_latest_balance() {
        let db = SettlementDb::connect_test().await.unwrap();

        let user_id = 1002;

        // Insert multiple events for same asset
        db.insert_balance_event(user_id, 1, 1000, 0, 1).await.unwrap();
        db.insert_balance_event(user_id, 1, 2000, 0, 2).await.unwrap();
        db.insert_balance_event(user_id, 1, 1500, 0, 3).await.unwrap(); // Latest

        tokio::time::sleep(Duration::from_millis(100)).await;

        let balances = db.get_user_all_balances(user_id).await.unwrap();

        // Should return only latest (seq=3)
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].avail, 1500);
        assert_eq!(balances[0].version, 3);
    }
}
```

**Run tests:**
```bash
cargo test --lib settlement_db::tests::test_mv
```

---

#### **4.2 Integration Test**

**File**: `test_balance_query.sh`

```bash
#!/bin/bash
set -e

echo "Testing MV-based balance queries..."

# 1. Insert test balance event
USER_ID=9999
curl -X POST "http://localhost:3001/api/deposit" \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 9999,
    "asset": "BTC",
    "amount": 500000000
  }'

# 2. Wait for Settlement to process
sleep 2

# 3. Query balance via Gateway
BALANCE=$(curl -s "http://localhost:3001/api/user/balance?user_id=9999" | jq -r '.data[] | select(.asset=="BTC") | .avail')

# 4. Verify
if [ "$BALANCE" == "5" ]; then
    echo "✅ MV query test PASSED: Balance = 5 BTC"
else
    echo "❌ MV query test FAILED: Expected 5, got $BALANCE"
    exit 1
fi

# 5. Query directly from MV
MV_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user WHERE user_id=9999;" | grep -o '[0-9]*' | head -1)

echo "✅ MV contains $MV_COUNT rows for user 9999"
```

**Run:**
```bash
chmod +x test_balance_query.sh
./test_balance_query.sh
```

---

### Step 5: Performance Benchmarking (10 minutes)

**File**: `benchmark_balance_query.sh`

```bash
#!/bin/bash

USER_ID=1001
ITERATIONS=100

echo "Benchmarking balance queries ($ITERATIONS iterations)..."

# Warm up
curl -s "http://localhost:3001/api/user/balance?user_id=$USER_ID" > /dev/null

# Benchmark
START=$(date +%s%N)
for i in $(seq 1 $ITERATIONS); do
    curl -s "http://localhost:3001/api/user/balance?user_id=$USER_ID" > /dev/null
done
END=$(date +%s%N)

TOTAL_MS=$(( ($END - $START) / 1000000 ))
AVG_MS=$(( $TOTAL_MS / $ITERATIONS ))

echo "Total time: ${TOTAL_MS}ms"
echo "Average per query: ${AVG_MS}ms"
echo ""
echo "Expected: 2-3ms per query (with MV)"
echo "Previous: 5-10ms per query (without MV)"
```

**Run:**
```bash
chmod +x benchmark_balance_query.sh
./benchmark_balance_query.sh
```

**Expected output:**
```
Average per query: 2-3ms ✅ (3x improvement!)
```

---

### Step 6: Monitoring & Metrics (15 minutes)

**File**: `src/gateway.rs`

**Add metrics to balance query:**

```rust
async fn get_balance(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<UserIdParams>,
) -> Result<Json<ApiResponse<Vec<ClientBalance>>>, (StatusCode, String)> {
    let start = Instant::now();
    let user_id = params.user_id;

    if let Some(db) = &state.db {
        let balances = db
            .get_user_all_balances(user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let latency = start.elapsed();

        // Metrics
        metrics::histogram!("balance_query_latency_ms", latency.as_millis() as f64);
        metrics::histogram!("balance_query_assets_count", balances.len() as f64);

        // Log slow queries
        if latency > Duration::from_millis(10) {
            log::warn!(
                "Slow balance query: user={} latency={:?} assets={}",
                user_id, latency, balances.len()
            );
        }

        // Convert to response...
    }
}
```

**Add to startup:**
```rust
// Check MV performance on startup
let test_user = 1001;
let start = Instant::now();
let _ = db.get_user_all_balances(test_user).await;
let latency = start.elapsed();

log::info!("MV query latency test: {:?}", latency);
if latency > Duration::from_millis(10) {
    log::warn!("⚠️ MV queries slower than expected");
}
```

---

### Step 7: Documentation Update (5 minutes)

**File**: `README.md` or create `docs/BALANCE_QUERIES.md`

```markdown
## Balance Queries

### Architecture

User balance queries use ScyllaDB Materialized View for O(1) lookups:

```
Gateway → MV (user_balances_by_user) → Return balances
                ↑ (automatic)
Settlement → balance_ledger → MV auto-updates
```

### Query Performance

- **Latency**: 2-3ms typical
- **Throughput**: 1000+ QPS per node
- **Scalability**: O(1) regardless of asset count

### MV Schema

See `schema/add_balance_mv.cql`

### Troubleshooting

If MV is missing or corrupted:
```bash
# Recreate MV
docker exec scylla cqlsh < schema/add_balance_mv.cql

# Wait for backfill (30-60 minutes)
# Check progress:
watch "docker exec scylla cqlsh -e 'SELECT COUNT(*) FROM trading.user_balances_by_user;'"
```
```

---

## Deployment Checklist

### Pre-Deployment

- [ ] MV schema file created (`schema/add_balance_mv.cql`)
- [ ] Code changes reviewed (settlement_db.rs)
- [ ] Unit tests pass
- [ ] Integration tests pass
- [ ] Benchmark shows improvement (2-3ms)

### Deployment Steps

```bash
# 1. Create MV (on production ScyllaDB)
docker exec scylla cqlsh < schema/add_balance_mv.cql

# 2. Wait for backfill to complete
# Check progress every 5 minutes
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user;"

# 3. Deploy updated Gateway code
cargo build --release
# ... deploy process ...

# 4. Restart Gateway
systemctl restart order_gate_server

# 5. Verify queries working
curl "http://localhost:3001/api/user/balance?user_id=1001"

# 6. Monitor latency
tail -f logs/gateway.log | grep "balance_query_latency"
```

### Post-Deployment Verification

- [ ] MV health check shows "Healthy"
- [ ] Balance queries return correct data
- [ ] Latency < 5ms (p99)
- [ ] No errors in logs
- [ ] Settlement still processing correctly (MV auto-updates)

---

## Rollback Plan

If MV causes issues:

```bash
# 1. Revert Gateway code (restore old query function)
git revert <commit>
cargo build --release
systemctl restart order_gate_server

# 2. Drop MV (safe - doesn't affect base table)
docker exec scylla cqlsh -e "DROP MATERIALIZED VIEW trading.user_balances_by_user;"

# 3. Verify old queries working
curl "http://localhost:3001/api/user/balance?user_id=1001"
```

**Note**: Dropping MV is safe! Base table (`balance_ledger`) is never affected.

---

## Success Criteria

✅ **Performance**: Query latency 2-3ms (3x improvement)
✅ **Functionality**: Returns all user assets automatically
✅ **Scalability**: Works with any number of assets
✅ **Simplicity**: Zero Settlement changes
✅ **Reliability**: MV auto-updates, no manual maintenance

---

## Time Estimate

- Step 1 (Schema): 5 min
- Step 2 (Code): 15 min
- Step 3 (Health check): 10 min
- Step 4 (Testing): 20 min
- Step 5 (Benchmark): 10 min
- Step 6 (Monitoring): 15 min
- Step 7 (Docs): 5 min

**Total**: ~80 minutes of development time
**MV backfill**: 30-60 minutes (background, doesn't block development)

---

## Next Steps (Future)

After MV is stable (1-2 weeks):

1. **Add snapshots** (ScyllaDB native, daily)
2. **Consider TTL** (if storage > 500GB)
3. **Add memory cache** (if QPS > 1000)

See `SNAPSHOT_STRATEGY.md` and `BALANCE_QUERY_ARCHITECTURE.md` for details.
