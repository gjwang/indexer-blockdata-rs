# Query API Optimization Guide

**Version:** 1.0
**Date:** 2025-12-10
**Status:** 📋 Planning Document

---

## 🎯 Overview

This document outlines optimization strategies for the frequently-queried trade and order history APIs:
- `GET /api/v1/order/trades` - Trade history
- `GET /api/v1/order/history` - Order history

---

## 📊 Current Implementation

### **Architecture:**
```
Gateway → ScyllaDB (settled_trades table)
          ↓
     2 queries: buyer + seller
          ↓
     Merge in memory
          ↓
     Filter by symbol (client-side)
          ↓
     Aggregate by order_id (for order history)
```

### **Code Location:**
- API Handlers: `src/gateway.rs:491-659`
- DB Queries: `src/db/settlement_db.rs:602-641`

### **Current Queries:**
```sql
-- Query 1: Trades as buyer
SELECT * FROM settled_trades
WHERE buyer_user_id = ?
LIMIT ?;

-- Query 2: Trades as seller
SELECT * FROM settled_trades
WHERE seller_user_id = ?
LIMIT ?;
```

---

## ⚠️ Performance Issues

### **1. No Symbol Index**
**Problem:**
- Fetches ALL user trades regardless of symbol
- Client-side filtering: `if t.base_asset_id == base && t.quote_asset_id == quote`
- Inefficient for users with trades across many symbols

**Impact:**
- User with 10K trades querying 1 symbol: fetches 10K, filters to ~500
- Wasted network bandwidth
- High memory usage
- Slow response time

**Metrics:**
- Records Scanned: O(n) where n = total user trades
- Response Time: 50-200ms depending on data volume

---

### **2. Double Query + Merge**
**Problem:**
- 2 separate DB queries per request
- Merge and sort in application memory
- May fetch 2×limit records to return limit results

**Impact:**
- 2× network round-trips
- Inefficient sorting: O(n log n) in memory
- No DB-level optimization

---

### **3. Order Aggregation**
**Problem:**
- For order history: builds HashMap per request
- Aggregates trades → orders in memory
- No pre-computation or caching

**Impact:**
```rust
// This runs on EVERY request:
let mut orders_map: HashMap<u64, OrderAgg> = HashMap::new();
for t in trades {
    let entry = orders_map.entry(t.buy_order_id).or_insert(...);
    entry.quantity += t.quantity;
    // ... more aggregation
}
```

- O(n) memory per request
- Redundant computation
- No caching benefits

---

## ✅ Optimization Strategies

### **Phase 1: Database Schema (High Impact, Medium Effort)**

#### **1.1 Add Materialized Views for Symbol Queries**

**Goal:** Enable efficient queries by (user, symbol)

**Schema:**
```sql
-- Buyer trades by symbol
CREATE MATERIALIZED VIEW trades_by_buyer_symbol AS
  SELECT * FROM settled_trades
  WHERE buyer_user_id IS NOT NULL
    AND base_asset_id IS NOT NULL
    AND quote_asset_id IS NOT NULL
  PRIMARY KEY ((buyer_user_id, base_asset_id, quote_asset_id), settled_at, trade_id)
  WITH CLUSTERING ORDER BY (settled_at DESC);

-- Seller trades by symbol
CREATE MATERIALIZED VIEW trades_by_seller_symbol AS
  SELECT * FROM settled_trades
  WHERE seller_user_id IS NOT NULL
    AND base_asset_id IS NOT NULL
    AND quote_asset_id IS NOT NULL
  PRIMARY KEY ((seller_user_id, base_asset_id, quote_asset_id), settled_at, trade_id)
  WITH CLUSTERING ORDER BY (settled_at DESC);
```

**New Query Function:**
```rust
pub async fn get_trades_by_user_symbol(
    &self,
    user_id: u64,
    base_asset_id: u32,
    quote_asset_id: u32,
    limit: i32
) -> Result<Vec<MatchExecData>> {
    // Single efficient query per role
    let buyer_trades = self.session.query(
        "SELECT * FROM trades_by_buyer_symbol
         WHERE buyer_user_id = ?
           AND base_asset_id = ?
           AND quote_asset_id = ?
         LIMIT ?",
        (user_id as i64, base_asset_id as i32, quote_asset_id as i32, limit)
    ).await?;

    let seller_trades = self.session.query(
        "SELECT * FROM trades_by_seller_symbol
         WHERE seller_user_id = ?
           AND base_asset_id = ?
           AND quote_asset_id = ?
         LIMIT ?",
        (user_id as i64, base_asset_id as i32, quote_asset_id as i32, limit)
    ).await?;

    // Merge and return
    merge_and_sort(buyer_trades, seller_trades, limit)
}
```

**Benefits:**
- ✅ Filters at DB level (not in memory)
- ✅ Pre-sorted by time (clustering order)
- ✅ Only fetches relevant symbol trades
- ✅ 10-50× faster for multi-symbol users

**Trade-offs:**
- ⚠️ Storage: 2× MVs (one per role)
- ⚠️ Write amplification: updates propagate to MVs
- ⚠️ Initial backfill time

**Estimated Impact:**
- Response time: 50-200ms → 10-50ms
- Records scanned: 10,000 → 500 (for 5% symbol ratio)

---

#### **1.2 Pre-Aggregated Order History Table**

**Goal:** Store orders (not trades) for instant queries

**Schema:**
```sql
CREATE TABLE order_history (
    user_id bigint,
    symbol_id int,
    order_id bigint,
    side tinyint,              -- 0=Buy, 1=Sell
    order_type tinyint,        -- 0=Market, 1=Limit
    price bigint,              -- Price in satoshis
    total_qty bigint,          -- Original order quantity
    filled_qty bigint,         -- Filled quantity (incremental)
    status tinyint,            -- 0=New, 1=PartialFill, 2=Filled, 3=Cancelled
    created_at bigint,         -- Order creation timestamp
    updated_at bigint,         -- Last update timestamp
    PRIMARY KEY ((user_id, symbol_id), updated_at, order_id)
) WITH CLUSTERING ORDER BY (updated_at DESC);

-- Index for order_id lookup
CREATE INDEX order_history_by_order_id ON order_history (order_id);
```

**Update Logic (Settlement Service):**
```rust
async fn on_trade_settled(&self, trade: &MatchExecData) -> Result<()> {
    // Update buyer's order
    self.upsert_order_history(OrderUpdate {
        user_id: trade.buyer_user_id,
        order_id: trade.buy_order_id,
        symbol_id: get_symbol_id(trade.base_asset_id, trade.quote_asset_id),
        side: Side::Buy,
        filled_qty_delta: trade.quantity,
        updated_at: trade.settled_at,
    }).await?;

    // Update seller's order
    self.upsert_order_history(OrderUpdate {
        user_id: trade.seller_user_id,
        order_id: trade.sell_order_id,
        symbol_id: get_symbol_id(trade.base_asset_id, trade.quote_asset_id),
        side: Side::Sell,
        filled_qty_delta: trade.quantity,
        updated_at: trade.settled_at,
    }).await?;

    Ok(())
}

async fn upsert_order_history(&self, update: OrderUpdate) -> Result<()> {
    // Use UPSERT with lightweight transaction or counter column
    self.session.query(
        "UPDATE order_history
         SET filled_qty = filled_qty + ?,
             updated_at = ?,
             status = ?
         WHERE user_id = ?
           AND symbol_id = ?
           AND order_id = ?",
        (update.filled_qty_delta, update.updated_at,
         calculate_status(...), update.user_id, update.symbol_id, update.order_id)
    ).await
}
```

**Query Function:**
```rust
pub async fn get_order_history(
    &self,
    user_id: u64,
    symbol_id: u32,
    limit: i32
) -> Result<Vec<OrderHistory>> {
    let rows = self.session.query(
        "SELECT * FROM order_history
         WHERE user_id = ? AND symbol_id = ?
         LIMIT ?",
        (user_id as i64, symbol_id as i32, limit)
    ).await?;

    parse_order_history_rows(rows)
}
```

**Benefits:**
- ✅ No aggregation at query time
- ✅ Single query (not 2×trades + aggregation)
- ✅ Real-time order status tracking
- ✅ Supports partial fills, cancellations
- ✅ 50-100× faster than current approach

**Trade-offs:**
- ⚠️ Additional table to maintain
- ⚠️ Write overhead on each trade
- ⚠️ Need to handle order creation (from ME/UBSCore)

**Estimated Impact:**
- Response time: 50-200ms → 5-20ms
- Complexity: O(n) aggregation → O(1) lookup

---

### **Phase 2: Caching Layer (Low Effort, High Impact)**

#### **2.1 Redis Cache for Hot Data**

**Goal:** Sub-millisecond responses for frequently-queried data

**Architecture:**
```
Gateway → Redis Cache → ScyllaDB
           ↓ hit          ↓ miss
        Return        Query + Cache
```

**Implementation:**
```rust
use redis::AsyncCommands;

pub struct CachedTradeHistory {
    db: SettlementDb,
    redis: redis::aio::Connection,
}

impl CachedTradeHistory {
    pub async fn get_trade_history(
        &mut self,
        user_id: u64,
        symbol: &str,
        limit: i32
    ) -> Result<Vec<Trade>> {
        let cache_key = format!("trades:{}:{}", user_id, symbol);

        // Try cache first
        if let Ok(cached) = self.redis.get::<_, String>(&cache_key).await {
            if let Ok(trades) = serde_json::from_str(&cached) {
                log::debug!("Cache hit: {}", cache_key);
                return Ok(trades);
            }
        }

        // Cache miss: query DB
        log::debug!("Cache miss: {}", cache_key);
        let trades = self.db.get_trades_by_user_symbol(
            user_id,
            parse_symbol(symbol)?,
            limit
        ).await?;

        // Cache for 60 seconds
        let serialized = serde_json::to_string(&trades)?;
        let _: () = self.redis.set_ex(&cache_key, serialized, 60).await?;

        Ok(trades)
    }
}
```

**Cache Invalidation Strategies:**

**Option A: TTL-based (Simple)**
```rust
// Cache expires after 60 seconds
redis.set_ex(key, value, 60).await?;
```
- ✅ Simple, no coordination
- ⚠️ Stale data for up to 60s

**Option B: Event-driven (Accurate)**
```rust
// Invalidate on trade settlement
async fn on_trade_settled(&self, trade: &MatchExecData) {
    let keys = vec![
        format!("trades:{}:*", trade.buyer_user_id),
        format!("trades:{}:*", trade.seller_user_id),
    ];
    for key in keys {
        redis.del(&key).await?;
    }
}
```
- ✅ Always fresh data
- ⚠️ More complex

**Option C: Hybrid (Recommended)**
```rust
// Short TTL + invalidation on major events
redis.set_ex(key, value, 10).await?; // 10s TTL
// + invalidate on settlement
```

**Benefits:**
- ✅ <1ms response time for cached data
- ✅ 90%+ cache hit rate for active users
- ✅ Reduces DB load by 10-100×
- ✅ Easy to implement

**Trade-offs:**
- ⚠️ Requires Redis infrastructure
- ⚠️ Potential cache invalidation complexity
- ⚠️ Memory cost (mitigated by TTL)

**Estimated Impact:**
- Response time: 10-50ms → <1ms (cache hit)
- DB load reduction: 90%+
- Cost: ~$50/month for Redis (AWS ElastiCache)

---

#### **2.2 Application-Level Cache**

**For smaller deployments without Redis:**

```rust
use moka::future::Cache;
use std::time::Duration;

lazy_static! {
    static ref TRADE_CACHE: Cache<String, Vec<Trade>> = Cache::builder()
        .max_capacity(10_000)
        .time_to_live(Duration::from_secs(60))
        .build();
}

pub async fn get_trade_history_cached(
    db: &SettlementDb,
    user_id: u64,
    symbol: &str,
    limit: i32
) -> Result<Vec<Trade>> {
    let cache_key = format!("{}:{}", user_id, symbol);

    // Try cache
    if let Some(cached) = TRADE_CACHE.get(&cache_key).await {
        return Ok(cached);
    }

    // Query DB
    let trades = db.get_trades_by_user_symbol(user_id, symbol, limit).await?;

    // Cache
    TRADE_CACHE.insert(cache_key, trades.clone()).await;

    Ok(trades)
}
```

**Benefits:**
- ✅ No external dependencies
- ✅ Simple to implement
- ✅ Good for single-instance deployments

**Trade-offs:**
- ⚠️ Not shared across instances
- ⚠️ Lost on restart
- ⚠️ Memory limited by instance size

---

### **Phase 3: Query Optimization (Quick Wins)**

#### **3.1 Add Pagination**

**Current:**
```rust
let limit = params.limit.unwrap_or(100);
```

**Improved:**
```rust
#[derive(Deserialize)]
struct HistoryParams {
    user_id: u64,
    symbol: String,
    limit: Option<i32>,
    cursor: Option<String>, // For pagination
}

// Validate and cap limit
let limit = params.limit.unwrap_or(20).min(100); // Max 100

// Use cursor for pagination
let cursor = params.cursor.as_deref();
let (trades, next_cursor) = db.get_trades_paginated(
    user_id, symbol, limit, cursor
).await?;

// Response includes next cursor
#[derive(Serialize)]
struct PaginatedResponse<T> {
    data: Vec<T>,
    next_cursor: Option<String>,
    has_more: bool,
}
```

**Benefits:**
- ✅ Prevents large scans
- ✅ Better mobile experience
- ✅ Reduces memory usage

---

#### **3.2 Add Response Compression**

```rust
use tower_http::compression::CompressionLayer;

let app = Router::new()
    .route("/api/v1/order/trades", get(get_trade_history))
    .layer(CompressionLayer::new()); // gzip/br
```

**Benefits:**
- ✅ 60-80% bandwidth reduction
- ✅ Faster for slow connections
- ✅ Minimal CPU overhead

---

#### **3.3 Implement Rate Limiting**

```rust
use tower_governor::{governor::GovernorConfig, GovernorLayer};

let governor_conf = GovernorConfig::default()
    .per_second(10) // 10 req/sec per user
    .burst_size(20);

let app = Router::new()
    .route("/api/v1/order/trades", get(get_trade_history))
    .layer(GovernorLayer { config: governor_conf });
```

**Benefits:**
- ✅ Protects against abuse
- ✅ Fair resource allocation
- ✅ Prevents DB overload

---

## 📈 Performance Comparison

| Metric | Current | +MV | +Pre-Agg | +Cache |
|--------|---------|-----|----------|--------|
| **DB Queries/Request** | 2 | 2 | 1 | 0 (hit) / 1 (miss) |
| **Records Scanned** | 10,000 | 500 | 1 | 0 |
| **Memory/Request** | O(n) | O(m) | O(1) | O(1) |
| **Response Time (p50)** | 100ms | 25ms | 10ms | <1ms |
| **Response Time (p99)** | 300ms | 75ms | 30ms | 5ms |
| **Throughput (QPS)** | 100 | 500 | 1000 | 10,000+ |
| **DB Load (%)** | 100% | 40% | 20% | 5% |

Where:
- n = total user trades
- m = symbol-specific trades (typically n/20)

---

## 🗓️ Implementation Roadmap

### **Immediate (Week 1):**
1. ✅ Add pagination (limit cap)
2. ✅ Add response compression
3. ✅ Add basic rate limiting
4. ✅ Monitor current performance metrics

**Estimated Effort:** 2-4 hours
**Impact:** 20% improvement

---

### **Short-term (Month 1):**
1. ⏳ Implement Materialized Views
2. ⏳ Update query functions
3. ⏳ Add backfill script
4. ⏳ Deploy and monitor

**Estimated Effort:** 2-3 days
**Impact:** 5× improvement

---

### **Medium-term (Quarter 1):**
1. ⏳ Design order_history table
2. ⏳ Implement Settlement updates
3. ⏳ Add order status tracking
4. ⏳ Migrate historical data

**Estimated Effort:** 1-2 weeks
**Impact:** 10× improvement

---

### **Long-term (When Needed):**
1. ⏳ Deploy Redis cluster
2. ⏳ Implement cache layer
3. ⏳ Add cache warming
4. ⏳ Tune eviction policies

**Estimated Effort:** 1 week
**Impact:** 100× improvement (cache hits)
**Trigger:** QPS > 1000 or latency SLA issues

---

## 📊 Monitoring & Metrics

### **Key Metrics to Track:**

```rust
// Add to each API handler
let start = Instant::now();

// ... query logic ...

let duration = start.elapsed();
metrics::histogram!("api.trade_history.duration_ms", duration.as_millis());
metrics::counter!("api.trade_history.requests", 1);
metrics::gauge!("api.trade_history.active_users", active_count);
```

**Dashboard:**
- Request latency (p50, p95, p99)
- Throughput (QPS)
- Error rate
- Cache hit rate (when applicable)
- DB query time breakdown
- Records scanned per query

### **Alerts:**
- p99 latency > 500ms
- Error rate > 1%
- Cache hit rate < 80% (when cache enabled)
- QPS approaching capacity

---

## 🎯 Decision Matrix

**When to optimize:**

| Scenario | Recommended Action |
|----------|-------------------|
| QPS < 100 | Current implementation OK |
| QPS 100-500 | Add pagination + compression |
| QPS 500-1000 | Implement MVs |
| QPS 1000-5000 | Add pre-aggregated tables |
| QPS > 5000 | Add Redis cache layer |
| Latency SLA miss | Immediate: cache, Long-term: all above |
| Storage cost concern | Evaluate MV trade-offs carefully |

---

## 🔗 References

- **Current Code:**
  - `src/gateway.rs:491-659` - API handlers
  - `src/db/settlement_db.rs:602-641` - DB queries

- **Related Docs:**
  - `BALANCE_QUERY_ARCHITECTURE.md` - MV implementation example
  - `MV_IMPLEMENTATION_PLAN.md` - MV guide

- **External:**
  - [ScyllaDB Materialized Views](https://docs.scylladb.com/stable/cql/mv.html)
  - [Redis Caching Patterns](https://redis.io/docs/manual/patterns/)
  - [API Rate Limiting Best Practices](https://cloud.google.com/architecture/rate-limiting-strategies)

---

*Created: 2025-12-10*
*Status: Planning Document*
*Next Review: When QPS > 500 or latency issues arise*
