# UBSCore Architecture: Professional Review

**Reviewer**: Senior Exchange Systems Architect
**Date**: 2024-12-08
**Scope**: Complete UBSCore design documentation

---

## Executive Assessment

**Overall Grade: A- (Production-Ready with Noted Improvements)**

This architecture represents a **well-designed, industry-standard approach** to building a high-frequency exchange. The design correctly identifies the key bottlenecks (persistence latency, balance contention, network jitter) and applies proven patterns from world-class exchanges (LMAX Disruptor, Aeron messaging, pre-trade risk isolation).

The documentation is comprehensive and demonstrates deep understanding of the trade-offs involved in HFT system design.

---

## Strengths (What You Got Right)

### 1. ✅ Correct Topology: Gateway → UBSCore → ME

This is the **only correct architecture** for a high-throughput exchange. By validating balances BEFORE the matching engine:
- ME operates at maximum speed (no balance lookups)
- Invalid orders are rejected immediately
- DDoS protection is built-in (empty wallets can't flood ME)

**Matches**: LMAX, Coinbase, Kraken, Binance

### 2. ✅ Synchronous WAL with O_DIRECT

The decision to use synchronous WAL persistence with O_DIRECT is **critical and correct**:
- Prevents "Ghost Money" (spending money that doesn't exist)
- O_DIRECT eliminates OS page cache jitter
- CRC32 framing enables crash recovery
- Group commit amortizes fsync cost

**This is non-negotiable for a financial system.**

### 3. ✅ Hot Path / Cold Path Separation

The speculative execution model is **exactly right**:
- Hot path (Aeron/SHM) enables sub-µs capital recycling
- Cold path (Redpanda) is the source of truth
- Reconciliation handles edge cases
- Timeout-based cleanup prevents stale credits

**This is how you achieve both speed AND correctness.**

### 4. ✅ Island Chain Pattern for Multi-Market

Separating UBSCore_Spot and UBSCore_Futures is **essential**:
- Futures math (margins, Greeks, liquidation) is CPU-heavy
- Massive futures liquidation cascade won't affect Spot
- Each market can scale independently
- Inter-Island Ferry via Redpanda is safe and auditable

### 5. ✅ Aeron (UDP/IPC) Over TCP Alternatives

The choice of Aeron for internal messaging is correct:
- No TCP head-of-line blocking
- Consistent latency profile
- Shared memory for co-located components
- Built-in term buffers for replay

### 6. ✅ Pre-Allocated WAL Rotation

The "Next Bullet" strategy for log rotation is a **professional-grade detail** that many designs miss:
- File creation is O(ms), unacceptable in hot path
- Pre-allocation removes fragmentation
- Background janitor ensures readiness

---

## Areas for Improvement (Critical Gaps)

### 1. ✅ Order ID Deduplication via halfULID + IndexSet (Final Design)

**Design Choice**: halfULID embeds timestamp + IndexSet for single-structure O(1) cache.

**Strategy: IndexSet (Single Structure, Ordered + O(1))**

```
┌────────────────────────────────────────────────────────────────────┐
│   CACHE_SIZE = 10,000        ← Max entries before force eviction    │
│   MAX_TIME_DRIFT = 3 sec     ← Accept window                        │
└────────────────────────────────────────────────────────────────────┘

IndexSet: Ordered insertion + O(1) lookup in ONE structure
├── Maintains insertion order (oldest at front)
├── O(1) contains() check
├── O(1) pop() from front
└── No double memory like Ring Buffer + HashSet
```

**Implementation (O(1) Amortized, Single Structure)**:

```rust
use indexmap::IndexSet;

const CACHE_SIZE: usize = 10_000;          // Max entries
const MAX_TIME_DRIFT_MS: u64 = 3_000;      // 3 second window

struct DeduplicationGuard {
    // Single structure: ordered + O(1) lookup
    cache: IndexSet<HalfUlid>,

    // Eviction boundary (high water mark)
    min_allowed_ts: u64,
}

impl DeduplicationGuard {
    fn new() -> Self {
        Self {
            cache: IndexSet::with_capacity(CACHE_SIZE),
            min_allowed_ts: 0,
        }
    }

    fn check_and_record(&mut self, order_id: HalfUlid) -> Result<(), RejectReason> {
        let now = current_time_ms();
        let order_ts = order_id.timestamp_ms();

        // 1. TIME CHECK
        if now - order_ts > MAX_TIME_DRIFT_MS {
            return Err(RejectReason::OrderTooOld);
        }
        if order_ts > now + 1_000 {
            return Err(RejectReason::FutureTimestamp);
        }
        if order_ts < self.min_allowed_ts {
            return Err(RejectReason::OrderTooOld);
        }

        // 2. DUPLICATE CHECK (O(1) lookup)
        if self.cache.contains(&order_id) {
            return Err(RejectReason::DuplicateOrderId);
        }

        // 3. EVICT IF FULL (pop oldest from front)
        if self.cache.len() >= CACHE_SIZE {
            if let Some(evicted) = self.cache.pop() {
                self.min_allowed_ts = self.min_allowed_ts.max(evicted.timestamp_ms());
            }
        }

        // 4. INSERT (append to back)
        self.cache.insert(order_id);

        Ok(())
    }
}
```

**Why IndexSet is Best**:

| Property | IndexSet | Ring Buffer + HashSet |
|----------|----------|----------------------|
| **Structures** | 1 | 2 (double memory) |
| **Memory** | ~200 KB | ~240 KB |
| **Lookup** | O(1) | O(1) |
| **Eviction** | O(1) pop() | O(1) overwrite |
| **Simplicity** | ✅ Simple | Complex |

**Why This Works**:

| Check | Action |
|-------|--------|
| `> 3 sec ago` | Reject `OrderTooOld` |
| `< min_allowed_ts` | Reject (evicted boundary) |
| In IndexSet | Reject `DuplicateOrderId` |
| Cache full | Pop oldest, update boundary |

**Memory Usage**:

| Setting | Value |
|---------|-------|
| IndexSet | 10,000 × ~20 bytes = ~200 KB |
| **Single structure** | **No double storage!** |

**⚠️ Critical: Replay Mode Exception**

During WAL replay, skip ALL time-based checks:

```rust
fn check_order_id(&mut self, order_id: HalfUlid) -> Result<(), RejectReason> {
    if self.is_replay_mode {
        return Ok(()); // WAL is pre-validated
    }
    self.dedup_guard.check_and_record(order_id)
}
```

**Why Skip Checks During Replay?**

```
Normal Mode:
  now = wall_clock_time()
  if now - order_ts > 5 sec → REJECT

Replay Mode Problem:
  WAL entry from yesterday: order_ts = 2024-12-07 14:00:00
  Current time:             now      = 2024-12-08 15:00:00
  Difference: 25 hours! → Would be REJECTED (WRONG!)
```

**Replay Rules**:
1. **Cannot use `now()` as reference** - replayed orders have old timestamps
2. **WAL is pre-validated** - orders in WAL already passed checks when first received
3. **Trust the WAL** - if it's in the log, it was valid
4. **No dedup needed** - WAL entries are already unique (written only once)

**Alternative: Simulated Time During Replay**

For testing/debugging, you can use the order's own timestamp as "now":

```rust
fn check_order_id(&mut self, order_id: HalfUlid) -> Result<(), RejectReason> {
    let now = if self.is_replay_mode {
        order_id.timestamp_ms()  // Use order's timestamp as "now"
    } else {
        current_time_ms()        // Use wall clock
    };

    // Rest of checks use this "now"...
}
```

**Summary**:
- ✅ Simple: Two constants (MAX_TIME_DRIFT, EXPIRE_TOLERANCE)
- ✅ Fast: Time check first (CPU only), O(1) amortized eviction
- ✅ Exact: Zero false positives (IndexSet lookup)
- ✅ Self-cleaning: No background thread, inline eviction
- ✅ Bounded: Memory = throughput × time_window



### 2. ⚠️ Missing: Graceful Degradation Under Load

**Current Design**: No explicit backpressure or load shedding.

**Problem**: Under extreme load, what happens?
- Gateway queues grow unbounded?
- UBSCore rejects orders silently?
- System crashes?

**Recommended Fix**:
```rust
impl UBSCore {
    fn accept_order(&mut self, order: Order) -> Result<(), RejectReason> {
        // 1. Check queue depth
        if self.pending_queue.len() > HIGH_WATER_MARK {
            self.metrics.increment("order.rejected.backpressure");
            return Err(RejectReason::SystemBusy);
        }

        // 2. Apply per-user rate limiting
        if self.rate_limiter.is_exceeded(order.user_id) {
            return Err(RejectReason::RateLimited);
        }

        // Continue normal processing...
    }
}
```

### 3. ⚠️ Missing: Explicit Consistency Model

**Current Design**: Mentions "eventual consistency" for speculative balances but doesn't define the guarantees.

**Recommended Addition**: Document explicit guarantees:

| Operation | Consistency | Guarantee |
|-----------|-------------|-----------|
| Balance Check | Read-Your-Own-Writes | See your own speculative credits |
| Order Placement | Linearizable | Single-threaded per user shard |
| Capital Recycling | Eventual | Hot path within 100µs, confirmed within 10ms |
| Cross-Shard Transfer | Sequential | Decrement-before-increment via Kafka |

### 4. ⚠️ Missing: Detailed Failure Modes

**Current Design**: Mentions Ghost Money handling but lacks exhaustive failure matrix.

**Recommended Addition**:

| Failure | Detection | Recovery | RTO |
|---------|-----------|----------|-----|
| UBSCore crash | Heartbeat timeout | Replay WAL from snapshot | 5-30s |
| ME crash | Heartbeat timeout | New ME, cancel open orders | 10-60s |
| Redpanda partition down | Consumer lag alert | Failover to replica | <1s |
| Network partition (UBS↔ME) | Message timeout | Reject new orders, hold state | Immediate |
| Hot path loss (UDP drop) | Cold path provides truth | Graceful, no action needed | N/A |
| Snapshot corruption | CRC validation | Use older snapshot + more WAL | 5-60s |

### 5. ⚠️ Missing: Multi-Asset Order Cost Calculation

**Current Design**: `order.cost` is treated as a single value.

**Problem**: Orders may involve multiple assets:
- Spot: Pay USDT, receive BTC
- Futures: Lock USDT as margin, affect BTC position

**Recommended Fix**:
```rust
struct OrderCost {
    debits: Vec<(AssetId, u64)>,   // Assets to debit
    credits: Vec<(AssetId, u64)>,  // Assets to credit (on fill)
    margin_requirement: Option<u64>, // For futures
}

impl UBSCore {
    fn validate_order(&self, order: &Order) -> Result<OrderCost, RejectReason> {
        let cost = self.calculate_cost(order)?;

        for (asset, amount) in &cost.debits {
            let balance = self.get_balance(order.user_id, *asset)?;
            if balance.available() < *amount {
                return Err(RejectReason::InsufficientBalance { asset: *asset });
            }
        }

        Ok(cost)
    }
}
```

### 6. ⚠️ Missing: Observability & Metrics

**Current Design**: Minimal mention of metrics.

**Critical Metrics to Track**:
```rust
struct UBSCoreMetrics {
    // Latency
    balance_check_latency_ns: Histogram,
    wal_append_latency_ns: Histogram,
    wal_sync_latency_ns: Histogram,

    // Throughput
    orders_processed_per_second: Counter,
    trades_processed_per_second: Counter,

    // Health
    pending_queue_depth: Gauge,
    speculative_credits_count: Gauge,
    wal_bytes_written: Counter,

    // Errors
    rejections_by_reason: CounterVec,
    hot_path_cold_path_mismatch: Counter,
    stale_speculative_credits: Counter,
}
```

### 7. ⚠️ Potential Issue: Lock Contention in HashMap

**Current Design**: `HashMap<UserId, Account>` with mutable access.

**Problem**: With 1M+ users, hash collisions and cache line contention become real.

**Recommended Fix**:
```rust
// Option A: Sharded HashMap
struct ShardedAccounts {
    shards: [RwLock<HashMap<UserId, Account>>; 256],
}

impl ShardedAccounts {
    fn get_shard(&self, user_id: UserId) -> &RwLock<HashMap<UserId, Account>> {
        &self.shards[(user_id % 256) as usize]
    }
}

// Option B: Lock-free with crossbeam
use crossbeam_skiplist::SkipMap;
struct Accounts {
    map: SkipMap<UserId, AtomicAccount>,
}
```

---

## Security Concerns

### 1. ⚠️ Integer Overflow in Balance Arithmetic

**Current Code**:
```rust
account.available -= order.cost;
account.frozen += order.cost;
```

**Risk**: If `order.cost > account.available`, this underflows (panic in debug, wrap in release).

**Fix**:
```rust
account.available = account.available.checked_sub(order.cost)
    .ok_or(RejectReason::InsufficientBalance)?;
account.frozen = account.frozen.checked_add(order.cost)
    .ok_or(RejectReason::InternalError)?; // This shouldn't happen
```

### 2. ⚠️ Validate Order Cost Externally

**Current Design**: `order.cost` comes from the Gateway.

**Risk**: Malicious Gateway could send incorrect cost.

**Fix**: UBSCore should calculate cost internally:
```rust
impl UBSCore {
    fn calculate_order_cost(&self, order: &Order) -> u64 {
        match order.side {
            Side::Buy => order.price * order.quantity / PRICE_DECIMALS,
            Side::Sell => order.quantity, // Selling the base asset
        }
    }
}
```

---

## Performance Optimizations (Nice to Have)

### 1. SIMD for Batch Processing

For processing 50+ orders per batch:
```rust
use std::simd::*;

fn batch_balance_check(balances: &[u64], costs: &[u64]) -> [bool; 64] {
    let bal = u64x64::from_slice(balances);
    let cost = u64x64::from_slice(costs);
    (bal >= cost).to_array()
}
```

### 2. CPU Pinning

For critical threads:
```rust
use core_affinity;

fn start_ubs_core() {
    let core_ids = core_affinity::get_core_ids().unwrap();
    core_affinity::set_for_current(core_ids[0]); // Pin to core 0

    // Run the hot loop
}
```

### 3. Huge Pages for Account HashMap

For large state:
```rust
// Use jemallocator with huge pages
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

// Configure huge pages
// echo 1024 > /proc/sys/vm/nr_hugepages
```

---

## Final Recommendations

### Must Fix Before Production

1. **Add checked arithmetic** for all balance operations
2. **Implement backpressure** with explicit load shedding
3. **Add comprehensive metrics** for observability
4. **Document failure modes** and recovery procedures
5. **Implement bounded deduplication** window

### Should Fix (High Priority)

6. Calculate order cost internally, not from Gateway
7. Add explicit consistency guarantees documentation
8. Implement sharded HashMap for scale
9. Add per-user rate limiting

### Nice to Have (Optimization)

10. SIMD batch processing
11. CPU pinning for hot threads
12. Huge pages for large state
13. io_uring for async disk I/O

---

## Verdict

This architecture is **ready for production** with the noted improvements. The core design decisions are correct and match industry best practices. The documentation quality is exceptional.

**Estimated Development Time to Production**:
- Core UBSCore: 2-3 months
- Full system integration: 4-6 months
- Production hardening: 2-3 additional months

**Total**: 8-12 months with a senior team of 3-5 engineers.

---

*"The architecture is sound. Now build it."*
