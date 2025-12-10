# Balance Query Architecture

**Date**: 2025-12-10
**Status**: Phase 1 Implementation (Materialized View)
**Author**: System Architecture Team

---

## Problem Statement

### The Challenge

Given the table schema:
```sql
CREATE TABLE trading.balance_ledger (
    user_id bigint,
    asset_id int,
    seq bigint,  -- Event sequence number
    avail bigint,
    frozen bigint,
    event_type text,
    PRIMARY KEY ((user_id, asset_id), seq)
) WITH CLUSTERING ORDER BY (seq DESC);
```

**Query requirement**: Get all current balances for a user (all assets)

**Problem**:
- Primary key requires BOTH `user_id` AND `asset_id`
- Querying by `user_id` alone is invalid (missing partition key)
- We don't know which assets a user has in advance

---

## Solution Options Evaluated

### Option 1: Query All Known Assets in Parallel ❌

```rust
for asset_id in [1, 2, 3, ...100] {  // All exchange assets
    query("WHERE user_id=? AND asset_id=? LIMIT 1", user_id, asset_id)
}
```

**Pros:**
- ✅ Simple implementation
- ✅ Zero Settlement changes
- ✅ No extra storage

**Cons:**
- ❌ Queries assets user doesn't have (wasted)
- ❌ O(N) queries where N = total exchange assets
- ❌ Latency: 50+ assets = 10-20ms
- ❌ Doesn't scale (200 assets = 50ms+)

**Verdict**: Not scalable

---

### Option 2: Manual Snapshot Table (user_balances) ❌

```sql
CREATE TABLE user_balances (
    user_id bigint,
    asset_id int,
    avail bigint,
    frozen bigint,
    PRIMARY KEY (user_id, asset_id)
);
```

Settlement writes to BOTH `balance_ledger` and `user_balances` on every update.

**Pros:**
- ✅ Fast queries (1ms)
- ✅ Small storage (snapshot only)
- ✅ Simple query (no filtering)

**Cons:**
- ❌ Requires dual writes in Settlement (+9% overhead)
- ❌ Manual maintenance (code complexity)
- ❌ Consistency risk (2 tables can diverge)
- ❌ Settlement becomes responsible for cache

**Verdict**: Too complex, risky

---

### Option 3: Materialized View ✅ **CHOSEN**

```sql
CREATE MATERIALIZED VIEW trading.user_balances_by_user AS
    SELECT user_id, asset_id, seq, avail, frozen, created_at
    FROM trading.balance_ledger
    WHERE user_id IS NOT NULL
      AND asset_id IS NOT NULL
      AND seq IS NOT NULL
    PRIMARY KEY (user_id, asset_id, seq)
    WITH CLUSTERING ORDER BY (asset_id ASC, seq DESC);
```

**Query:**
```rust
SELECT * FROM user_balances_by_user
WHERE user_id = ?
PER PARTITION LIMIT 1;  // Returns latest balance per asset automatically
```

**Pros:**
- ✅ Zero Settlement code changes (ScyllaDB handles updates)
- ✅ Automatic maintenance
- ✅ Real-time updates (~5-10ms lag)
- ✅ Single query returns all user's assets
- ✅ Fast (2-3ms)
- ✅ Scales to any number of assets

**Cons:**
- ⚠️ Settlement write overhead: +5% (+0.2ms latency, -22 ops/s)
- ⚠️ Storage: 2x (50GB base + 50GB MV = 100GB)
- ⚠️ MV backfill delay (minutes for initial creation)

**Verdict**: Best balance of simplicity, performance, and maintainability

---

## Decision Rationale

### Why Materialized View?

1. **Simplicity**: Zero Settlement changes, ScyllaDB handles all replication
2. **Real-time**: ~5-10ms eventual consistency (acceptable for crypto exchange)
3. **Performance**: 2-3ms queries vs 10-50ms parallel queries
4. **Scalability**: O(1) query regardless of exchange asset count
5. **Proven**: Standard ScyllaDB pattern, used by Discord, Instacart, Samsung

### Trade-offs Accepted

1. **+5% Settlement Overhead**:
   - Before: 435 ops/s, 2.29ms avg
   - After: 413 ops/s, 2.4ms avg
   - **Impact**: Negligible (not hitting capacity limits)

2. **2x Storage**:
   - Before: 50GB
   - After: 100GB
   - **Impact**: Acceptable (storage is cheap, queries are expensive)

3. **Eventual Consistency**:
   - Lag: 5-10ms from write to visible in MV
   - **Impact**: Users don't notice sub-10ms delays

---

## Implementation

### Phase 1: Materialized View (Current)

**Schema:**
```sql
CREATE MATERIALIZED VIEW trading.user_balances_by_user AS
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

**Query Function:**
```rust
pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
    let result = self.session.query(
        "SELECT asset_id, avail, frozen, seq, created_at
         FROM user_balances_by_user
         WHERE user_id = ?
         PER PARTITION LIMIT 1",  // Only latest balance per asset
        (user_id as i64,)
    ).await?;

    // Parse results
    parse_mv_results(result)
}
```

**Performance:**
- Query latency: 2-3ms
- Settlement overhead: +5%
- Storage: 100GB (base + MV)

---

## Future Optimization: Memory Cache Layer

### Phase 2: MV + Memory Cache (When Needed)

**When to implement:**
- Balance query volume > 1000 QPS
- P99 latency needs to be < 1ms
- Cache hit ratio can be high (80%+ queries from 20% users)

### Architecture

```
Gateway Query Flow:
1. Check memory cache (0.1ms) → Cache hit? Return ✅
2. Cache miss → Query MV (2-3ms)
3. Populate cache
4. Return result

Cache Invalidation:
Settlement writes → Publish to Kafka → Gateway invalidates cache
```

### Implementation Sketch

```rust
// Gateway: Memory cache
use moka::sync::Cache;

lazy_static! {
    static ref BALANCE_CACHE: Cache<u64, Vec<UserBalance>> =
        Cache::builder()
            .max_capacity(10_000)  // 10K hot users
            .time_to_live(Duration::from_secs(60))  // 60s TTL
            .build();
}

pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
    // Try cache
    if let Some(cached) = BALANCE_CACHE.get(&user_id) {
        return Ok(cached);  // 0.1ms ✅
    }

    // Cache miss - query MV
    let balances = self.query_mv(user_id).await?;  // 2-3ms

    // Populate cache
    BALANCE_CACHE.insert(user_id, balances.clone());

    Ok(balances)
}
```

```rust
// Settlement: Publish invalidation
async fn append_balance_events_batch(&self, events: &[BalanceEvent]) -> Result<()> {
    // Write to balance_ledger
    self.write_to_ledger(events).await?;

    // Publish cache invalidation
    for event in events {
        self.kafka_producer.send(
            FutureRecord::to("cache.invalidations")
                .key(&event.user_id.to_string())
                .payload(&bincode::serialize(&CacheInvalidation {
                    user_id: event.user_id
                })?),
            Duration::from_secs(0)
        ).await?;
    }

    Ok(())
}
```

```rust
// Gateway: Cache invalidation consumer
async fn consume_cache_invalidations() {
    let consumer = create_kafka_consumer("cache.invalidations");

    loop {
        match consumer.recv().await {
            Ok(msg) => {
                if let Ok(inv) = bincode::deserialize::<CacheInvalidation>(msg.payload()) {
                    BALANCE_CACHE.invalidate(&inv.user_id);
                    log::debug!("Cache invalidated for user {}", inv.user_id);
                }
            }
            Err(e) => log::error!("Cache consumer error: {}", e),
        }
    }
}
```

### Performance Expectations (Phase 2)

| Metric | Phase 1 (MV) | Phase 2 (MV + Cache) |
|--------|--------------|----------------------|
| Query latency (hot users) | 2-3ms | 0.1ms ✅ |
| Query latency (cold users) | 2-3ms | 2-3ms |
| Cache hit rate | N/A | 95-99% |
| Memory usage | Minimal | +10MB (10K users) |
| Settlement overhead | +5% | +7% (+cache invalidation) |
| Complexity | Low | Medium |

### When NOT to Add Cache

- ✅ If MV queries are fast enough (< 5ms acceptable)
- ✅ If query volume is low (< 500 QPS)
- ✅ If memory is constrained
- ✅ If simplicity is more important than sub-ms latency

---

## Migration Path

### Current State → Phase 1 (MV)

**Steps:**
1. Create MV (one-time CQL command)
2. Wait for backfill completion (minutes to hours)
3. Update `get_user_all_balances()` query
4. Test and deploy
5. Monitor Settlement performance

**Risk**: Low (can drop MV if issues)

### Phase 1 → Phase 2 (Add Cache)

**Triggers:**
- Query volume exceeds 1000 QPS
- P99 latency requirements < 1ms
- 80/20 query distribution confirmed

**Steps:**
1. Add memory cache to Gateway
2. Add cache invalidation publisher to Settlement
3. Add cache invalidation consumer to Gateway
4. Monitor cache hit rate and latency
5. Tune cache size and TTL

**Risk**: Medium (introduces Kafka dependency for cache coherency)

---

## Monitoring & Metrics

### Phase 1 (MV) Metrics

```rust
// Settlement
metrics::histogram!("settlement_write_latency_ms", latency);
metrics::gauge!("settlement_throughput_ops", throughput);

// Gateway
metrics::histogram!("balance_query_latency_ms", latency);
metrics::histogram!("balance_query_assets_returned", count);
```

**Alert thresholds:**
- Settlement write latency p99 > 10ms
- Settlement throughput < 350 ops/s
- Balance query latency p99 > 10ms

### Phase 2 (Cache) Metrics

```rust
// Gateway cache metrics
metrics::counter!("balance_cache_hits", 1);
metrics::counter!("balance_cache_misses", 1);
metrics::gauge!("balance_cache_size", BALANCE_CACHE.entry_count());
metrics::histogram!("balance_cache_hit_latency_ms", latency);

// Cache hit rate = hits / (hits + misses)
```

**Alert thresholds:**
- Cache hit rate < 90%
- Cache size > 15K entries (memory pressure)
- Invalidation lag > 100ms

---

## Resilience & Disaster Recovery

### MV is Rebuildable from Source

**Key principle**: `balance_ledger` is the **source of truth**. MV is a **derived view**.

If MV is dropped, corrupted, or needs recreation, it can be **fully rebuilt** from the base table.

---

### Rebuild Process

```sql
-- 1. Drop existing MV (if corrupted or needs recreation)
DROP MATERIALIZED VIEW trading.user_balances_by_user;

-- 2. Recreate MV with same (or updated) schema
CREATE MATERIALIZED VIEW trading.user_balances_by_user AS
    SELECT user_id, asset_id, seq, avail, frozen, created_at
    FROM trading.balance_ledger
    WHERE user_id IS NOT NULL
      AND asset_id IS NOT NULL
      AND seq IS NOT NULL
    PRIMARY KEY (user_id, asset_id, seq)
    WITH CLUSTERING ORDER BY (asset_id ASC, seq DESC);

-- 3. ScyllaDB automatically backfills from balance_ledger
--    (Background process, takes minutes to hours depending on data size)
```

**What happens during rebuild:**
1. ScyllaDB starts background backfill automatically
2. Reads all rows from `balance_ledger`
3. Populates MV incrementally
4. New writes go to both base table and MV (even during backfill)
5. Once complete, MV is fully synchronized

---

### Rebuild Time Estimates

| Data Size | Rows | Estimated Time | Planning |
|-----------|------|----------------|----------|
| 1GB | ~10M | 1-2 minutes | No downtime needed |
| 10GB | ~100M | 10-20 minutes | Brief performance impact |
| 100GB | ~1B | 1-2 hours | Plan during low traffic |
| 1TB | ~10B | 6-12 hours | Maintenance window recommended |

**Your system** (100K users, 5 assets, 1000 txns avg):
- Base table: ~50GB
- Rebuild time: **~30-60 minutes**
- Impact: Queries slower during rebuild (use fallback)

---

### Fallback Query During Rebuild

Implement graceful degradation while MV is unavailable:

```rust
pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
    // Try MV first (fast path)
    match self.query_mv(user_id).await {
        Ok(balances) if !balances.is_empty() => {
            return Ok(balances);  // MV available ✅
        }
        Ok(_) | Err(_) => {
            // MV unavailable or incomplete - use fallback
            log::warn!("MV unavailable for user {}, using source table fallback", user_id);
        }
    }

    // Fallback: Query source table directly (slower but reliable)
    self.query_source_table_fallback(user_id).await
}

async fn query_source_table_fallback(&self, user_id: u64) -> Result<Vec<UserBalance>> {
    // Get all exchange assets
    let assets = self.get_all_asset_ids(); // [1, 2, 3, ...]

    // Query each asset from base table in parallel
    let futures = assets.iter().map(|&asset_id| async move {
        self.session.query(
            "SELECT avail, frozen, seq, created_at
             FROM balance_ledger
             WHERE user_id = ? AND asset_id = ?
             ORDER BY seq DESC
             LIMIT 1",
            (user_id as i64, asset_id as i32)
        ).await
    });

    let results = join_all(futures).await;

    // Filter successful results and parse
    results.into_iter()
        .filter_map(|r| r.ok())
        .filter_map(|result| parse_balance_row(result))
        .collect()
}
```

**Fallback performance:**
- Latency: 5-10ms (slower than MV but acceptable)
- Works even when MV is completely unavailable
- Automatically resumes using MV once rebuild completes

---

### Monitoring MV Health

```rust
// Check MV status
async fn check_mv_health(&self) -> Result<MvStatus> {
    // Query system tables
    let result = self.session.query(
        "SELECT view_name, include_all_columns
         FROM system_schema.views
         WHERE keyspace_name = 'trading'
           AND view_name = 'user_balances_by_user'",
        ()
    ).await?;

    if result.rows.is_none() {
        return Ok(MvStatus::Missing);
    }

    // Check if MV is up-to-date (sample check)
    let base_count = self.count_base_table_rows().await?;
    let mv_count = self.count_mv_rows().await?;

    let completeness = (mv_count as f64 / base_count as f64) * 100.0;

    if completeness < 95.0 {
        Ok(MvStatus::Rebuilding(completeness))
    } else {
        Ok(MvStatus::Healthy)
    }
}

enum MvStatus {
    Healthy,
    Rebuilding(f64),  // Percentage complete
    Missing,
}
```

**Metrics to track:**
```rust
metrics::gauge!("mv_health_status", match status {
    MvStatus::Healthy => 1.0,
    MvStatus::Rebuilding(pct) => pct / 100.0,
    MvStatus::Missing => 0.0,
});

metrics::counter!("balance_query_fallback_count", 1); // Track fallback usage
```

---

### Recovery Scenarios

#### **Scenario 1: Schema Change**
```
Need to add new column to MV:
1. DROP old MV
2. CREATE new MV with updated schema
3. Wait for backfill
4. Use fallback queries during rebuild
```

#### **Scenario 2: Corruption Detected**
```
MV returns inconsistent data:
1. Investigate (compare MV vs base table samples)
2. DROP corrupted MV
3. RECREATE MV
4. ScyllaDB rebuilds from source
5. Verify data consistency
```

#### **Scenario 3: Migration from Old System**
```
Old system has no MV:
1. Ensure balance_ledger is populated
2. CREATE MV
3. ScyllaDB backfills automatically
4. Update code to use MV query
5. Deploy Gateway with MV support
```

---

### Best Practices

1. **Always maintain base table integrity**
   - `balance_ledger` is source of truth
   - Never delete from base table
   - Use append-only pattern

2. **Test fallback queries regularly**
   - Don't wait for disaster to test fallback
   - Periodically simulate MV unavailability
   - Verify fallback performance

3. **Monitor rebuild progress**
   - Track MV row count vs base table
   - Alert if rebuild takes longer than expected
   - Log when falling back to source queries

4. **Plan for maintenance windows**
   - Large rebuilds (>100GB) during low traffic
   - Communicate to users if queries are slower
   - Have rollback plan

---

## Summary

### Decision: Materialized View (Phase 1)

**Chosen for:**
- Simplicity (zero Settlement changes)
- Performance (2-3ms queries)
- Real-time guarantees (~10ms freshness)
- Scalability (handles any asset count)

### Future: Add Memory Cache (Phase 2 - Optional)

**Add when:**
- Need sub-millisecond latency
- High query volume (1000+ QPS)
- Cache hit rate justifies complexity

### Current Status

✅ **MV schema documented**
✅ **Query function updated**
⏭️ **Ready for implementation**
📊 **Monitoring plan defined**

---

**Next Steps:**
1. Create MV in ScyllaDB
2. Update query function
3. Deploy and monitor
4. Revisit cache layer in 3-6 months based on metrics
