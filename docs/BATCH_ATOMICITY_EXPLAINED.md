# BATCH Atomicity Across Multiple Partitions

## The Question

**One trade updates**:
- User 1001 (buyer): BTC balance, USDT balance
- User 2005 (seller): BTC balance, USDT balance

**That's 2 different partitions (2 different user_ids)!**

How can this be atomic?

---

## Understanding ScyllaDB Partitions

### Partition Distribution

```
ScyllaDB Cluster:
┌─────────────────────────────────────────────────────────┐
│ Node 1                                                   │
│  Partition: user_id=1001                                │
│    - asset_id=1 (BTC):  available=1.5                  │
│    - asset_id=2 (USDT): available=10000                │
├─────────────────────────────────────────────────────────┤
│ Node 2                                                   │
│  Partition: user_id=2005                                │
│    - asset_id=1 (BTC):  available=5.0                  │
│    - asset_id=2 (USDT): available=50000                │
└─────────────────────────────────────────────────────────┘
```

**The trade**:
```rust
// BTC/USDT trade: 0.1 BTC @ 100,000 USDT
// Buyer (1001): -10,000 USDT, +0.1 BTC
// Seller (2005): +10,000 USDT, -0.1 BTC

let mut batch = BatchStatement::new(BatchType::Logged);

// Updates to Node 1 (user 1001)
batch.append("UPDATE user_balances SET available = available + 10000000
              WHERE user_id = 1001 AND asset_id = 1");  // +0.1 BTC
batch.append("UPDATE user_balances SET available = available - 1000000000
              WHERE user_id = 1001 AND asset_id = 2");  // -10000 USDT

// Updates to Node 2 (user 2005)
batch.append("UPDATE user_balances SET available = available - 10000000
              WHERE user_id = 2005 AND asset_id = 1");  // -0.1 BTC
batch.append("UPDATE user_balances SET available = available + 1000000000
              WHERE user_id = 2005 AND asset_id = 2");  // +10000 USDT

session.batch(&batch, ()).await?;
```

**Question**: How is this atomic across **2 different nodes**?

---

## How LOGGED Batch Provides Atomicity

### Step-by-Step Execution

#### 1. **Coordinator Receives Batch**

```
Client → Coordinator Node (e.g., Node 1)
```

The coordinator is the node that receives the batch request.

#### 2. **Write to Batch Log (Durable)**

```
Coordinator writes to batchlog table:
┌─────────────────────────────────────────────────────────┐
│ Batch Log (replicated across cluster)                   │
├─────────────────────────────────────────────────────────┤
│ batch_id: uuid-1234                                     │
│ statements: [                                           │
│   "UPDATE user_balances ... user_id=1001 asset_id=1",  │
│   "UPDATE user_balances ... user_id=1001 asset_id=2",  │
│   "UPDATE user_balances ... user_id=2005 asset_id=1",  │
│   "UPDATE user_balances ... user_id=2005 asset_id=2"   │
│ ]                                                       │
│ status: PENDING                                         │
└─────────────────────────────────────────────────────────┘
```

**Critical**: Batch log is **replicated** (typically RF=2) and **durable**.

#### 3. **Execute Statements on Target Nodes**

```
Coordinator sends:
  - Statements 1,2 → Node 1 (user 1001)
  - Statements 3,4 → Node 2 (user 2005)
```

Each node executes its statements **independently**.

#### 4. **Wait for All Acknowledgments**

```
Node 1 → "Done" → Coordinator
Node 2 → "Done" → Coordinator
```

Coordinator waits for **all nodes** to acknowledge.

#### 5. **Mark Batch as Complete**

```
Coordinator updates batch log:
  status: PENDING → COMPLETE
```

#### 6. **Remove Batch Log Entry**

```
Batch log entry deleted (async)
```

---

## What Happens if There's a Crash?

### Scenario 1: Crash Before Batch Log Write

```
1. Client sends batch
2. ⚡ Coordinator crashes
3. Nothing written
```

**Result**: ✅ **No partial state** - batch never started

---

### Scenario 2: Crash After Batch Log, Before Execution

```
1. Batch log written (durable)
2. ⚡ Coordinator crashes before sending to nodes
3. Coordinator restarts
4. Reads batch log, finds PENDING batch
5. Re-executes batch
```

**Result**: ✅ **Batch completes** - replayed from log

---

### Scenario 3: Crash During Execution (Partial Completion)

```
1. Batch log written
2. Node 1 completes (user 1001 updated)
3. ⚡ Node 2 crashes (user 2005 NOT updated)
4. Coordinator detects failure
5. Batch log still shows PENDING
6. Coordinator retries
7. Node 1: Statements re-executed (idempotent)
8. Node 2: Statements executed
```

**Result**: ✅ **Eventually consistent** - batch completes after retry

---

### Scenario 4: Coordinator Crashes During Execution

```
1. Batch log written (replicated)
2. Coordinator sends to Node 1, Node 2
3. ⚡ Coordinator crashes
4. Another node becomes coordinator
5. New coordinator reads batch log
6. Finds PENDING batch
7. Re-executes batch
```

**Result**: ✅ **Batch completes** - another node takes over

---

## Key Insight: Batch Log is the Source of Truth

### Batch Log Table

```sql
CREATE TABLE system.batchlog (
    id uuid PRIMARY KEY,
    data blob,           -- Serialized batch statements
    written_at timestamp,
    version int
) WITH gc_grace_seconds = 0;
```

**Properties**:
- **Replicated** (RF=2 by default)
- **Durable** (written to disk)
- **Distributed** (across multiple nodes)

**Guarantee**:
> If batch log entry exists with status=PENDING, the batch **WILL** be executed, even if nodes crash.

---

## Why This Works Across Partitions

### Traditional Database (Single-Node)

```
BEGIN TRANSACTION;
  UPDATE user_balances SET ... WHERE user_id = 1001;
  UPDATE user_balances SET ... WHERE user_id = 2005;
COMMIT;
```

**Atomicity**: Single node controls entire transaction.

### ScyllaDB (Distributed)

```
Write to batch log (replicated, durable)
  ↓
Execute on multiple nodes
  ↓
Wait for all acks
  ↓
Mark batch as complete
```

**Atomicity**: Batch log ensures **eventual completion** even across crashes.

---

## Important Caveats

### 1. **Not True ACID Transactions**

ScyllaDB batches are **NOT** like SQL transactions:

❌ **No isolation**: Other clients can see partial state during execution
❌ **No rollback**: If one statement fails, others may still succeed
✅ **Eventual consistency**: Batch will complete eventually

### 2. **Idempotency Required**

Statements may be **re-executed** during recovery:

```rust
// ✅ Idempotent - safe to re-execute
UPDATE user_balances SET available = available + 100 WHERE user_id = 1001;

// ❌ NOT idempotent - would double-apply
UPDATE user_balances SET available = 200 WHERE user_id = 1001;
```

**Our balance updates are idempotent** because we use `+=` and `-=`.

### 3. **Performance Cost**

Batch log writes add overhead:
- Extra disk I/O
- Replication overhead
- ~2-3x slower than individual writes

---

## Practical Example

### Trade Settlement

```rust
// Trade: User 1001 buys 0.1 BTC from User 2005 for 10,000 USDT

let mut batch = BatchStatement::new(BatchType::Logged);

// 1. Insert trade (partition: trade_id)
batch.append("INSERT INTO settled_trades_btc_usdt (trade_id, ...) VALUES (?, ...)");

// 2. Buyer balances (partition: user_id=1001)
batch.append("UPDATE user_balances SET available = available + ?
              WHERE user_id = 1001 AND asset_id = 1");  // +0.1 BTC
batch.append("UPDATE user_balances SET available = available - ?
              WHERE user_id = 1001 AND asset_id = 2");  // -10000 USDT

// 3. Seller balances (partition: user_id=2005)
batch.append("UPDATE user_balances SET available = available - ?
              WHERE user_id = 2005 AND asset_id = 1");  // -0.1 BTC
batch.append("UPDATE user_balances SET available = available + ?
              WHERE user_id = 2005 AND asset_id = 2");  // +10000 USDT

// Execute atomically
session.batch(&batch, ()).await?;
```

**What happens**:
1. Batch log written (replicated)
2. Trade insert sent to node holding trade partition
3. User 1001 updates sent to node holding user 1001 partition
4. User 2005 updates sent to node holding user 2005 partition
5. Coordinator waits for all acks
6. Batch marked complete
7. ✅ **All 5 operations succeed or all eventually succeed**

---

## Summary

**How is it atomic across 2 users (2 partitions)?**

1. **Batch log** (replicated, durable) records all statements
2. **Coordinator** sends statements to appropriate nodes
3. **Waits for all acks** before marking complete
4. **If crash**: Batch log ensures statements are **replayed**
5. **Idempotent statements** allow safe re-execution

**Result**:
- ✅ **Eventual atomicity** - all statements will complete
- ✅ **Survives crashes** - batch log provides durability
- ⚠️ **Not instant** - may take time during recovery
- ⚠️ **Not isolated** - partial state may be visible briefly

**For settlement**: This is **sufficient** because:
- We can tolerate brief partial visibility
- Eventual consistency is acceptable
- Idempotency prevents double-application
- Batch log ensures no data loss

**This is why LOGGED batch works for your use case!**
