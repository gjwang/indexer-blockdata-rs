# Balance Snapshot & Recovery Strategy

**Date**: 2025-12-10
**Status**: Recommended Architecture (Phase 1)
**Decision**: ScyllaDB Native Snapshots with Eventually Consistent Model

---

## Executive Summary

**Chosen Approach**: ScyllaDB native snapshots without write pause (eventually consistent)

**Key Benefits**:
- ⚡ Instant snapshots (~100ms, atomic per-node)
- 💾 Zero storage overhead (hard links)
- 🔄 Constant recovery time (O(1), even after years)
- 🚀 Zero downtime
- 🛠️ Simple implementation (built-in ScyllaDB feature)

**Trade-off Accepted**:
- Snapshot captures state across ~500ms window (eventually consistent across nodes)
- Acceptable because user balances are independent partitions

---

## The Problem: Unbounded Replay

### Why Snapshots Are Essential

Without snapshots, recovery requires replaying **all history**:

```
Year 1: 100M events → Replay: 1 hour
Year 5: 500M events → Replay: 5 hours
Year 10: 1B events → Replay: 12 hours
Year 100: 10B events → Replay: IMPOSSIBLE
```

**Event sourcing without snapshots = unbounded replay time** ❌

With snapshots:
```
Any age: Load snapshot + replay recent events → ~10 minutes ✅
```

**Snapshot = Compressed history up to point T**
**Recent events = Delta since T**
**Full state = Snapshot + Delta**

Recovery time is **constant** regardless of system age! 🎯

---

## Architecture Overview

### Phase 1: ScyllaDB Native Snapshots (Current Recommendation)

```
Settlement Service → balance_ledger (no TTL initially)
                  ↓
              ScyllaDB → MV (automatic, real-time)
                  ↓
         Daily 3 AM → nodetool snapshot (100ms, atomic)
                  ↓
          Optional → Upload snapshot to S3 (compliance)

Gateway Queries ← MV (2-3ms queries)

Recovery Path:
1. Restore from snapshot → 2-5 minutes
2. MV auto-rebuilds from ledger → 30-60 minutes
Total: ~30-60 minutes
```

---

## Snapshot Consistency Models

### Option 1: Paused Writes (Strict Consistency) ⏸️

**Implementation:**
```rust
async fn create_strictly_consistent_snapshot() -> Result<()> {
    // 1. Stop accepting new writes
    set_readonly_mode(true).await?;

    // 2. Wait for in-flight requests to drain
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. Create snapshot (all nodes at exact same logical time)
    nodetool_snapshot_all_nodes("snapshot_name").await?;

    // 4. Resume writes
    set_readonly_mode(false).await?;

    Ok(())
}
```

**Characteristics:**
- ✅ Perfect point-in-time consistency
- ✅ Exact global balance at snapshot instant
- ❌ 100-200ms write pause (downtime)
- ❌ Coordination complexity

**When to use:**
- Regulatory requirement for exact point-in-time audit
- Cross-user atomic operations require consistency
- Financial compliance mandates it

---

### Option 2: Logical Snapshot Markers 🏷️

**Implementation:**
```rust
async fn create_logical_snapshot() -> Result<()> {
    // 1. Insert logical marker BEFORE snapshot
    let start_seq = get_next_global_seq();
    insert_marker("snapshot_start", start_seq).await?;

    // 2. Create physical snapshot (no pause)
    nodetool_snapshot_all_nodes("snapshot_name").await?;

    // 3. Insert logical marker AFTER snapshot
    let end_seq = get_next_global_seq();
    insert_marker("snapshot_end", end_seq).await?;

    // 4. Store metadata
    save_snapshot_metadata(SnapshotMetadata {
        name: snapshot_name,
        start_seq,
        end_seq,
        timestamp: now(),
    }).await?;

    Ok(())
}

// Recovery uses markers to determine replay range
async fn recover_from_logical_snapshot() -> Result<()> {
    let metadata = load_snapshot_metadata("latest")?;

    // Replay events AFTER end_seq (no duplicates)
    replay_events_since(metadata.end_seq + 1).await?;

    Ok(())
}
```

**Characteristics:**
- ✅ No write pause (zero downtime)
- ✅ Clear replay boundaries (end_seq)
- ✅ Prevents double-application
- ⚠️ More complex implementation
- ⚠️ Still eventually consistent (500ms window)

**When to use:**
- Need precise replay semantics
- Want to track snapshot boundaries
- Building audit trail

---

### Option 3: Eventually Consistent (Simple) ✅ **CHOSEN**

**Implementation:**
```bash
# Cron job: Daily at 3 AM
0 3 * * * nodetool snapshot trading -t snapshot_$(date +\%Y\%m\%d)
```

```rust
async fn create_simple_snapshot() -> Result<()> {
    let snapshot_name = format!("snapshot_{}", today());

    // Single command - ScyllaDB handles everything
    let output = Command::new("nodetool")
        .args(&["snapshot", "trading", "-t", &snapshot_name])
        .output()?;

    if output.status.success() {
        log::info!("✅ Snapshot created: {}", snapshot_name);

        // Optional: Upload to S3 for compliance/backup
        tokio::spawn(upload_snapshot_to_s3(snapshot_name));
    }

    Ok(())
}
```

**Characteristics:**
- ✅ Dead simple (one command)
- ✅ Zero downtime
- ✅ Instant (~100ms)
- ✅ Built-in ScyllaDB feature
- ⚠️ Eventually consistent (500ms window across nodes)

**Recovery:**
```rust
async fn recover_from_snapshot() -> Result<()> {
    // 1. Restore snapshot
    nodetool_refresh("trading", "balance_ledger").await?;

    // 2. Each partition's seq-based replay handles consistency
    // (Seq numbers provide ordering within partition)

    // 3. MV rebuilds automatically
    recreate_mv().await?;

    Ok(())
}
```

**Why this works:**
1. **Independent partitions**: Each `(user_id, asset_id)` is isolated
2. **Seq-based ordering**: Within partition, seq provides total order
3. **Replay fixes inconsistencies**: Events applied by seq ensure correctness
4. **No cross-user atomicity**: User balances don't depend on each other

---

## Consistency Analysis

### What "Eventually Consistent" Means

```
3-node cluster snapshot:

3:00:00.000 AM - Snapshot command issued

Node 1: Snapshot at 3:00:00.100 (100ms)
Node 2: Snapshot at 3:00:00.250 (250ms) ← 150ms after Node 1
Node 3: Snapshot at 3:00:00.500 (500ms) ← 400ms after Node 1

Events during 500ms window:
- User 1001 (Node 1): Deposit 100 BTC at 3:00:00.050 → Captured ✅
- User 1002 (Node 2): Deposit 50 ETH at 3:00:00.150 → Captured ✅
- User 1003 (Node 3): Withdraw 10 BTC at 3:00:00.300 → Captured ✅

Result: Snapshot contains events from T0 to T0+500ms (not single instant)
```

### Is This Acceptable?

**YES, for this system!** Here's why:

#### 1. Partitions Are Independent
```
PRIMARY KEY: ((user_id, asset_id), seq)

Each partition lives on ONE node:
- User 1001 BTC: Node 1 only
- User 1002 ETH: Node 2 only
- No cross-partition dependencies

Within partition: Atomic snapshot ✅
Across partitions: Not needed ✅
```

#### 2. Seq Provides Per-Partition Ordering
```
After snapshot restore:

User 1001: BTC seq=1000 (from snapshot)
User 1002: ETH seq=2000 (from snapshot)

Replay events:
- User 1001: Apply seq > 1000 ✅
- User 1002: Apply seq > 2000 ✅

Each user's timeline is consistent!
```

#### 3. No Cross-User Transactions
```
Operations are:
- Deposits (single user) ✅
- Withdrawals (single user) ✅
- Trades (settlement updates both, seq-based replay fixes) ✅

NOT:
- Atomic transfers between users (would need strict consistency)
```

### When Strict Consistency IS Required

❌ **NOT acceptable if:**
- Cross-user atomic transfers
- Regulatory requirement for exact global balance at instant T
- Audit trail must show simultaneous snapshot
- Multi-partition transactions require atomicity

✅ **For this system:**
- User balances are independent ✅
- Seq-based replay ensures per-user consistency ✅
- Brief recovery inconsistency acceptable ✅

---

## How ScyllaDB Snapshots Work

### SSTable Immutability

```
ScyllaDB data structure:
/var/lib/scylla/data/trading/balance_ledger-abc123/
├── data-001.db  ← SSTable (IMMUTABLE)
├── data-002.db  ← SSTable (IMMUTABLE)
└── data-003.db  ← SSTable (IMMUTABLE)

New writes → Create NEW SSTable (data-004.db)
Old SSTables → Never modified

Snapshot = Hard links to existing SSTables
```

### Snapshot Creation

```bash
nodetool snapshot trading -t snapshot_2024_12_10

Creates:
/var/lib/scylla/data/trading/balance_ledger-abc123/snapshots/snapshot_2024_12_10/
├── data-001.db → ../../../../data-001.db  (hard link)
├── data-002.db → ../../../../data-002.db  (hard link)
└── data-003.db → ../../../../data-003.db  (hard link)

Time: ~100ms (just creates links)
Storage: 0 bytes initially (same inodes)
```

### Why It's Safe

1. **SSTables are immutable** → Hard links don't break
2. **Snapshot isolated** → Compaction won't delete linked files
3. **Atomic per-node** → Node-level consistency guaranteed
4. **No data copy** → Instant, regardless of data size

---

## Recovery Procedures

### Scenario 1: MV Corruption (Common)

```bash
# MV corrupted, but base table (balance_ledger) is fine

# 1. Drop corrupted MV
docker exec scylla cqlsh -e "DROP MATERIALIZED VIEW trading.user_balances_by_user;"

# 2. Recreate MV
docker exec scylla cqlsh -e "
CREATE MATERIALIZED VIEW trading.user_balances_by_user AS
    SELECT user_id, asset_id, seq, avail, frozen, created_at
    FROM trading.balance_ledger
    WHERE user_id IS NOT NULL AND asset_id IS NOT NULL
    PRIMARY KEY (user_id, asset_id, seq)
    WITH CLUSTERING ORDER BY (seq DESC);
"

# 3. ScyllaDB auto-backfills from balance_ledger
# Time: 30-60 minutes for 50GB

# 4. Verify
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user;"
```

**No snapshot needed!** Base table is source of truth.

---

### Scenario 2: Base Table Corruption (Rare)

```bash
# Base table corrupted, need to restore from snapshot

# 1. Stop ScyllaDB (maintenance mode)
docker exec scylla nodetool drain
docker stop scylla

# 2. Restore snapshot
docker exec scylla nodetool refresh trading balance_ledger

# 3. Restart ScyllaDB
docker start scylla

# 4. Recreate MV
docker exec scylla cqlsh -e "
DROP MATERIALIZED VIEW trading.user_balances_by_user;
CREATE MATERIALIZED VIEW trading.user_balances_by_user ...;
"

# Time: ~30-60 minutes total
```

---

### Scenario 3: Full Cluster Disaster

```bash
# All nodes lost, restore from S3 backup

# 1. Download snapshot from S3
aws s3 sync s3://backups/snapshots/2024-12-10/ /var/lib/scylla/data/

# 2. Restore tables
nodetool refresh trading balance_ledger
nodetool refresh trading user_balances_by_user

# 3. Verify data
cqlsh -e "SELECT COUNT(*) FROM trading.balance_ledger;"

# Time: ~1-2 hours (depending on S3 download)
```

---

## Storage Management

### Snapshot Retention Policy

```rust
// Keep rolling 7-day window
async fn cleanup_old_snapshots() -> Result<()> {
    let snapshots = list_snapshots().await?;

    for snapshot in snapshots {
        if snapshot.age_days() > 7 {
            // Delete local snapshot
            Command::new("nodetool")
                .args(&["clearsnapshot", "-t", &snapshot.name])
                .output()?;

            log::info!("Deleted old snapshot: {}", snapshot.name);
        }
    }

    Ok(())
}
```

### Storage Growth

```
Day 0: snapshot_2024_12_10 created
       Storage: 0 bytes (hard links to existing data)

Day 1: New writes create new SSTables (500MB)
       snapshot_2024_12_10 still points to old SSTables
       Storage: 500MB (old data + new data)

Day 2: snapshot_2024_12_11 created
       Compaction hasn't run yet
       Storage: 500MB + 300MB = 800MB

Day 7: Compaction runs, old SSTables merged
       snapshot_2024_12_10 prevents deletion of old SSTables
       Storage: ~3GB (incremental growth)

Day 8: Delete snapshot_2024_12_10
       Old SSTables can now be deleted
       Storage: ~2GB

Steady state: ~2-3GB for 7-day rolling window
```

---

## Future Optimizations

### When to Reconsider Eventually Consistent Model

**Triggers for upgrading to Option 1 or 2:**

1. **Regulatory Compliance**
   - Auditor requires exact point-in-time snapshots
   - Financial regulations mandate strict consistency
   - Legal requirement for simultaneous balance capture

2. **Cross-User Atomicity**
   - Implement peer-to-peer transfers
   - Add multi-user transactions
   - Need global consistency guarantees

3. **Discovered Bugs**
   - Recovery produces incorrect balances
   - Eventually consistent window causes issues
   - Business logic depends on atomic snapshots

4. **Compliance Archive**
   - Need to prove exact state at time T
   - Audit trail requires strict timestamps
   - Regulatory reporting demands it

### Migration Path

```
Phase 1 (Now): Eventually Consistent
- Simple nodetool snapshot
- Good enough for 95% of cases

Phase 2 (If needed): Logical Markers
- Add snapshot metadata tracking
- Still zero downtime
- Better replay semantics

Phase 3 (If required): Paused Writes
- Add readonly mode
- 100ms downtime acceptable at 3 AM
- Perfect consistency
```

---

## TTL Strategy (Future)

### Current: No TTL

```sql
-- balance_ledger: Keep all history
-- No default_time_to_live set
```

**When to add TTL:**
- Storage > 500GB
- Cost > acceptable threshold
- History beyond N years not needed for operations

### Future: Add TTL + Archive

```sql
-- Option A: 1-year operational window
ALTER TABLE trading.balance_ledger
WITH default_time_to_live = 31536000;  -- 1 year

-- Option B: 7-day hot window (aggressive)
ALTER TABLE trading.balance_ledger
WITH default_time_to_live = 604800;  -- 7 days
```

**With snapshots:**
- Daily snapshot captures state
- TTL deletes old events
- Recovery: Snapshot + events within TTL window
- Older history: Archive to S3 Glacier for compliance

---

## Monitoring & Alerting

### Critical Metrics

```rust
// Snapshot health
metrics::gauge!("snapshot_age_hours", last_snapshot_age.as_hours());
metrics::gauge!("snapshot_size_mb", snapshot_size_mb);
metrics::counter!("snapshot_failures_total", 1);

// Alert if snapshot older than 25 hours
if last_snapshot_age > Duration::from_hours(25) {
    alert_pagerduty("Snapshot job failed!");
}

// Alert if snapshot size anomaly (possible corruption)
let expected_size = previous_snapshot_size * 0.9..previous_snapshot_size * 1.2;
if !expected_size.contains(&current_snapshot_size) {
    alert_pagerduty("Snapshot size anomaly!");
}
```

### Recovery Testing

```rust
// Monthly recovery drill (automated)
async fn monthly_recovery_test() -> Result<()> {
    // 1. Create test cluster
    let test_cluster = create_test_cluster().await?;

    // 2. Restore yesterday's snapshot
    restore_snapshot(&test_cluster, "yesterday").await?;

    // 3. Replay recent events
    replay_events(&test_cluster).await?;

    // 4. Compare with production
    let diff = compare_with_production(&test_cluster).await?;

    // 5. Alert if mismatch
    if diff.mismatch_percent > 0.01 {
        alert_pagerduty(&format!("Recovery test failed: {}% mismatch", diff.mismatch_percent));
    }

    // 6. Cleanup
    destroy_test_cluster(&test_cluster).await?;

    Ok(())
}
```

---

## Implementation Checklist

### Phase 1: Basic Snapshots (Week 1)

```
✅ Set up daily cron job: nodetool snapshot
✅ Implement snapshot cleanup (7-day retention)
✅ Add snapshot age monitoring
✅ Document recovery procedures
✅ Test snapshot restore manually
```

### Phase 2: Automation (Week 2-3)

```
✅ Automate S3 upload (optional, for compliance)
✅ Add snapshot integrity checks
✅ Implement automated recovery testing
✅ Set up PagerDuty alerts
✅ Create runbook for on-call engineers
```

### Phase 3: Hardening (Month 2-3)

```
○ Add cross-region S3 replication
○ Implement compliance archive (if needed)
○ Monthly recovery drills
○ Performance tuning for large datasets
○ Consider TTL if storage growth becomes issue
```

---

## Decision Log

### Why Eventually Consistent?

**Date**: 2025-12-10

**Decision**: Use ScyllaDB native snapshots without write pause (Option 3)

**Rationale**:
1. User balances are independent partitions (no cross-user atomicity)
2. Seq-based replay provides per-partition consistency
3. 500ms consistency window is negligible for recovery scenarios
4. Simplicity and zero downtime outweigh theoretical inconsistency
5. Can upgrade to stricter model if requirements change

**Trade-offs Accepted**:
- Snapshot captures state across 500ms, not single instant
- Global balance total may be briefly inconsistent during recovery window
- Acceptable for operational recovery, might need stricter model for compliance

**When to Revisit**:
- Regulatory requirements change
- Cross-user transactions added
- Bugs discovered related to consistency window
- Business demands point-in-time accuracy

---

## References

- [ScyllaDB Snapshots Documentation](https://docs.scylladb.com/stable/operating-scylla/procedures/backup-restore/backup.html)
- [Event Sourcing Pattern](https://martinfowler.com/eaaDev/EventSourcing.html)
- [Cassandra Snapshot Consistency](https://cassandra.apache.org/doc/latest/cassandra/operating/backups.html)

---

**Next Review Date**: 2025-03-10 (3 months)
**Owner**: Infrastructure Team
**Status**: Approved for Production
