# Active Orders Implementation Guide

**Version:** 1.0
**Date:** 2025-12-10
**Status:** 🎯 Complete Design & Implementation Plan

---

## 📋 Table of Contents

1. [Overview](#overview)
2. [Phase 1: ScyllaDB Implementation (Current)](#phase-1-scylladb-implementation)
3. [Phase 2: ME Memory Query (Future Optimization)](#phase-2-me-memory-query)
4. [Performance Comparison](#performance-comparison)
5. [Implementation Roadmap](#implementation-roadmap)

---

## 🎯 Overview

Active orders are **open orders in the Matching Engine** that haven't been fully filled or cancelled.

### **Two Approaches:**

| Approach | Latency | Complexity | When |
|----------|---------|------------|------|
| **ScyllaDB** | 5-10ms | Low | **Current** (Phase 1) |
| **ME Memory** | <1ms | High | Future (Phase 2, if needed) |

**Decision:** Start with ScyllaDB (simpler, good enough), optimize with ME later if needed.

---

## 🗄️ Phase 1: ScyllaDB Implementation

### **Architecture**

```
┌─────────────────────────────────────────────────────────┐
│ Order Lifecycle in Database                             │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  1. Order Placed                                         │
│     ME → EngineOutput.order_placements                   │
│     → Settlement → DB.insert_active_order()              │
│                                                          │
│  2. Partial Fill                                         │
│     ME → EngineOutput.order_updates                      │
│     → Settlement → DB.update_filled_qty() [COUNTER]      │
│                                                          │
│  3. Completion                                           │
│     ME → EngineOutput.order_completions                  │
│     → Settlement → DB.update_status(Filled/Cancelled)    │
│                                                          │
│  4. Cleanup (Background)                                 │
│     Job → DELETE completed orders older than 10 min      │
│                                                          │
│  5. Query                                                │
│     Gateway → DB.get_active_orders(user, symbol)         │
│     → Returns orders with status IN (New, PartialFill)   │
└─────────────────────────────────────────────────────────┘
```

---

### **1. Database Schema**

```sql
-- Active orders table
CREATE TABLE IF NOT EXISTS active_orders (
    -- Partition key: (user_id, symbol_id) for O(1) queries
    user_id bigint,
    symbol_id int,

    -- Clustering columns: time-ordered
    created_at bigint,
    order_id bigint,

    -- Order details
    side tinyint,          -- 0=Buy, 1=Sell
    order_type tinyint,    -- 0=Market, 1=Limit
    price bigint,          -- Price in satoshis
    quantity bigint,       -- Total order quantity
    filled_qty counter,    -- ★ COUNTER for atomic updates

    -- Status
    status tinyint,        -- 0=New, 1=PartialFill, 2=Filled, 3=Cancelled
    updated_at bigint,

    PRIMARY KEY ((user_id, symbol_id), created_at, order_id)
) WITH CLUSTERING ORDER BY (created_at DESC, order_id DESC)
  AND comment = 'Active orders by user+symbol, counter for atomic fills'
  AND default_time_to_live = 86400;  -- Auto-expire after 24h

-- Lookup table for partition key resolution
CREATE TABLE IF NOT EXISTS order_lookup (
    order_id bigint PRIMARY KEY,
    user_id bigint,
    symbol_id int,
    created_at bigint
) WITH comment = 'Partition key lookup for order updates';

CREATE INDEX IF NOT EXISTS order_lookup_idx ON order_lookup (order_id);
```

**Key Design Decisions:**

1. **Counter Type for `filled_qty`** ⭐⭐⭐
   - Atomic increments
   - No race conditions
   - Perfect for accumulating fills

2. **Partition Key: (user_id, symbol_id)** ⭐⭐⭐
   - O(1) query for GET /api/v1/order/active
   - Matches API contract exactly
   - Even data distribution

3. **Lookup Table** ⭐⭐⭐
   - Updates need partition key
   - Can't update with just order_id
   - Small overhead, big benefit

4. **TTL: 24 hours** ⭐⭐
   - Auto-cleanup of old orders
   - Keeps table size manageable
   - No manual cleanup needed (backup)

---

### **2. Enhanced EngineOutput**

```rust
// File: src/engine_output.rs
pub struct EngineOutput {
    pub sequence: u64,
    pub trades: Vec<TradeOutput>,
    pub balance_updates: Vec<BalanceUpdate>,

    // ★ NEW: Order lifecycle events
    pub order_placements: Vec<OrderPlacement>,
    pub order_updates: Vec<OrderUpdate>,
    pub order_completions: Vec<OrderCompletion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderPlacement {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,
    pub order_type: u8,
    pub price: u64,
    pub quantity: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub order_id: u64,
    pub filled_qty_delta: u64,  // Increment amount
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderCompletion {
    pub order_id: u64,
    pub reason: CompletionReason,
    pub completed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionReason {
    Filled,
    Cancelled,
}
```

---

### **3. Matching Engine Updates**

```rust
// File: src/matching_engine_base.rs
impl MatchingEngine {
    pub fn place_order(&mut self, order: Order) -> EngineOutput {
        let mut output = EngineOutput::new(self.sequence);

        // 1. Record order placement
        output.order_placements.push(OrderPlacement {
            order_id: order.order_id,
            user_id: order.user_id,
            symbol_id: self.current_symbol,
            side: order.side as u8,
            order_type: order.order_type as u8,
            price: order.price,
            quantity: order.quantity,
            created_at: get_timestamp_ns(),
        });

        // 2. Try to match
        let trades = self.match_order_internal(&mut order);
        output.trades.extend(trades);

        // 3. If partially filled, record update
        if order.filled_quantity > 0 && order.filled_quantity < order.quantity {
            output.order_updates.push(OrderUpdate {
                order_id: order.order_id,
                filled_qty_delta: order.filled_quantity,
            });
        }

        // 4. If fully filled, record completion
        if order.filled_quantity >= order.quantity {
            output.order_completions.push(OrderCompletion {
                order_id: order.order_id,
                reason: CompletionReason::Filled,
                completed_at: get_timestamp_ns(),
            });
        }

        output
    }

    pub fn cancel_order(&mut self, order_id: u64) -> EngineOutput {
        let mut output = EngineOutput::new(self.sequence);

        // Remove from book
        if self.remove_order_from_book(order_id).is_ok() {
            output.order_completions.push(OrderCompletion {
                order_id,
                reason: CompletionReason::Cancelled,
                completed_at: get_timestamp_ns(),
            });
        }

        output
    }
}
```

---

### **4. Settlement Service: Atomic Batch Processing**

```rust
// File: src/bin/settlement_service.rs

/// Settle entire EngineOutput atomically
pub async fn settle_engine_output_atomic(
    db: &SettlementDb,
    output: &EngineOutput,
) -> Result<()> {
    let start = Instant::now();

    // Build single batch with ALL operations
    let mut batch = Batch::new(BatchType::Unlogged);
    let mut values = Vec::new();

    // 1. Insert new orders
    for order in &output.order_placements {
        batch.append_statement(INSERT_ACTIVE_ORDER_CQL);
        values.push((
            order.user_id as i64,
            order.symbol_id as i32,
            order.created_at,
            order.order_id as i64,
            order.side as i8,
            order.order_type as i8,
            order.price as i64,
            order.quantity as i64,
            OrderStatus::New as i8,
            order.created_at,
        ));

        // Also insert into lookup table
        batch.append_statement(INSERT_ORDER_LOOKUP_CQL);
        values.push((
            order.order_id as i64,
            order.user_id as i64,
            order.symbol_id as i32,
            order.created_at,
        ));

        // Cache partition key immediately
        db.cache_partition_key(
            order.order_id,
            order.user_id,
            order.symbol_id,
            order.created_at,
        );
    }

    // 2. Update filled quantities (COUNTER)
    for update in &output.order_updates {
        let pk = db.get_partition_key_cached(update.order_id)?;
        batch.append_statement(UPDATE_FILLED_QTY_CQL);
        values.push((
            update.filled_qty_delta as i64,
            pk.user_id as i64,
            pk.symbol_id as i32,
            pk.created_at,
            update.order_id as i64,
        ));
    }

    // 3. Mark completions
    for completion in &output.order_completions {
        let pk = db.get_partition_key_cached(completion.order_id)?;
        let status = match completion.reason {
            CompletionReason::Filled => OrderStatus::Filled,
            CompletionReason::Cancelled => OrderStatus::Cancelled,
        };
        batch.append_statement(UPDATE_STATUS_CQL);
        values.push((
            status as i8,
            completion.completed_at,
            pk.user_id as i64,
            pk.symbol_id as i32,
            pk.created_at,
            completion.order_id as i64,
        ));
    }

    // 4. Write trades (already implemented)
    for trade in &output.trades {
        // ... existing trade batch logic ...
    }

    // EXECUTE ENTIRE BATCH ATOMICALLY
    db.session.batch(&batch, values).await
        .context("Failed to settle engine output")?;

    let duration = start.elapsed();
    log::info!(
        "Settled seq={} in {}ms: {} orders, {} updates, {} completions, {} trades",
        output.sequence,
        duration.as_millis(),
        output.order_placements.len(),
        output.order_updates.len(),
        output.order_completions.len(),
        output.trades.len()
    );

    Ok(())
}
```

**★ Key Points:**
- ONE batch for entire EngineOutput
- Unlogged batch for performance (ME WAL is source of truth)
- Partition key caching for fast updates
- Prepared statements for efficiency

---

### **5. Database Functions**

```rust
// File: src/db/settlement_db.rs

use lru::LruCache;

pub struct SettlementDb {
    session: Arc<Session>,

    // ★ Partition key cache (critical for performance!)
    pk_cache: Arc<Mutex<LruCache<u64, OrderPartitionKey>>>,

    // Prepared statements
    insert_order_stmt: PreparedStatement,
    update_filled_stmt: PreparedStatement,
    update_status_stmt: PreparedStatement,
}

#[derive(Clone)]
struct OrderPartitionKey {
    user_id: u64,
    symbol_id: u32,
    created_at: i64,
}

impl SettlementDb {
    pub async fn new(session: Session) -> Result<Self> {
        // Prepare statements once
        let insert_order_stmt = session.prepare(INSERT_ACTIVE_ORDER_CQL).await?;
        let update_filled_stmt = session.prepare(UPDATE_FILLED_QTY_CQL).await?;
        let update_status_stmt = session.prepare(UPDATE_STATUS_CQL).await?;

        Ok(Self {
            session: Arc::new(session),
            pk_cache: Arc::new(Mutex::new(LruCache::new(100_000))),
            insert_order_stmt,
            update_filled_stmt,
            update_status_stmt,
        })
    }

    /// Cache partition key for fast lookups
    pub fn cache_partition_key(
        &self,
        order_id: u64,
        user_id: u64,
        symbol_id: u32,
        created_at: i64,
    ) {
        self.pk_cache.lock().unwrap().put(
            order_id,
            OrderPartitionKey { user_id, symbol_id, created_at }
        );
    }

    /// Get partition key (cached or DB)
    pub fn get_partition_key_cached(
        &self,
        order_id: u64,
    ) -> Result<OrderPartitionKey> {
        // Try cache first
        if let Some(pk) = self.pk_cache.lock().unwrap().get(&order_id) {
            return Ok(pk.clone());
        }

        // Cache miss: query lookup table
        let pk = self.get_partition_key_from_db(order_id).await?;
        self.pk_cache.lock().unwrap().put(order_id, pk.clone());
        Ok(pk)
    }

    async fn get_partition_key_from_db(&self, order_id: u64) -> Result<OrderPartitionKey> {
        let row = self.session.query(
            "SELECT user_id, symbol_id, created_at FROM order_lookup WHERE order_id = ?",
            (order_id as i64,)
        ).await?
        .first_row()?;

        let (user_id, symbol_id, created_at): (i64, i32, i64) = row.into_typed()?;
        Ok(OrderPartitionKey {
            user_id: user_id as u64,
            symbol_id: symbol_id as u32,
            created_at,
        })
    }

    /// Query active orders for user+symbol
    pub async fn get_active_orders(
        &self,
        user_id: u64,
        symbol_id: u32,
    ) -> Result<Vec<ActiveOrder>> {
        let result = self.session.query(
            "SELECT * FROM active_orders
             WHERE user_id = ? AND symbol_id = ?
             AND status IN (0, 1)",  // New or PartialFill
            (user_id as i64, symbol_id as i32)
        ).await?;

        let mut orders = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                orders.push(parse_active_order_row(row)?);
            }
        }

        Ok(orders)
    }
}
```

---

### **6. Gateway API**

```rust
// File: src/gateway.rs

async fn get_active_orders(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<ActiveOrderResponse>>>, (StatusCode, String)> {
    let user_id = params.user_id;
    let symbol_id = match state.symbol_manager.get_symbol_id(&params.symbol) {
        Some(id) => id,
        None => return Ok(Json(ApiResponse::success(vec![]))),
    };

    if let Some(db) = &state.db {
        // Query active orders from DB
        let active_orders = db
            .get_active_orders(user_id, symbol_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Convert to response format
        let response: Vec<ActiveOrderResponse> = active_orders
            .into_iter()
            .filter_map(|o| {
                let price_decimal = state.balance_manager.to_client_price(&params.symbol, o.price)?;
                let qty_decimal = state.balance_manager.to_client_amount(o.base_asset_id, o.quantity)?;
                let filled_decimal = state.balance_manager.to_client_amount(o.base_asset_id, o.filled_qty)?;

                Some(ActiveOrderResponse {
                    order_id: o.order_id.to_string(),
                    symbol: params.symbol.clone(),
                    side: if o.side == 0 { "BUY" } else { "SELL" }.to_string(),
                    order_type: if o.order_type == 0 { "Market" } else { "Limit" }.to_string(),
                    price: price_decimal,
                    quantity: qty_decimal,
                    filled_quantity: filled_decimal,
                    status: format!("{:?}", OrderStatus::from(o.status)),
                    created_at: o.created_at,
                })
            })
            .collect();

        Ok(Json(ApiResponse::success(response)))
    } else {
        Ok(Json(ApiResponse::success(vec![])))
    }
}
```

---

## 🚀 Phase 2: ME Memory Query (Future Optimization)

### **When to Implement:**
- QPS > 10,000 for active orders
- Latency requirement <1ms
- Need real-time order book state

### **Architecture**

```
┌──────────────────────────────────────────────────────┐
│ Direct Memory Query (Future)                          │
├──────────────────────────────────────────────────────┤
│                                                       │
│  Gateway → Aeron Query → ME (in-memory)              │
│                    ↓                                  │
│              OrderBook.get_user_orders()              │
│                    ↓                                  │
│              Aeron Response → Gateway                 │
│                                                       │
│  Latency: <1ms                                        │
│  Data: Always fresh from ME memory                    │
└──────────────────────────────────────────────────────┘
```

### **Implementation**

```rust
// File: src/matching_engine_base.rs
impl OrderBook {
    /// Query active orders for a user (in-memory)
    pub fn get_user_orders(&self, user_id: u64) -> Vec<ActiveOrderInfo> {
        let mut orders = Vec::new();

        // Scan buy side
        for order in &self.buy_orders {
            if order.user_id == user_id {
                orders.push(ActiveOrderInfo {
                    order_id: order.order_id,
                    side: Side::Buy,
                    price: order.price,
                    quantity: order.quantity,
                    filled_quantity: order.filled_quantity,
                    created_at: order.created_at,
                });
            }
        }

        // Scan sell side
        for order in &self.sell_orders {
            if order.user_id == user_id {
                orders.push(ActiveOrderInfo {
                    order_id: order.order_id,
                    side: Side::Sell,
                    price: order.price,
                    quantity: order.quantity,
                    filled_quantity: order.filled_quantity,
                    created_at: order.created_at,
                });
            }
        }

        orders
    }
}
```

**Performance:**
- Scan entire order book: O(n) where n = active orders in symbol
- Typical: 100-10K orders = <100μs
- Response time: <1ms total

**Trade-offs:**
- ✅ Ultra-low latency
- ⚠️ More complex (Aeron request-response)
- ⚠️ No historical data (only current state)

**See:** `ACTIVE_ORDER_QUERY_DESIGN.md` for full implementation details

---

## 📊 Performance Comparison

| Metric | ScyllaDB (Phase 1) | ME Memory (Phase 2) |
|--------|-------------------|---------------------|
| **Latency (p50)** | 5ms | <1ms |
| **Latency (p99)** | 20ms | 2ms |
| **Throughput** | 10K QPS | 100K+ QPS |
| **Implementation** | 2-3 hours | 6-8 hours |
| **Complexity** | Low | High |
| **Data Freshness** | ~10ms delay | Real-time |
| **Historical Queries** | Yes | No |
| **Audit Trail** | Built-in | Need DB anyway |
| **Recovery** | From DB | Need DB sync |

---

## 🗺️ Implementation Roadmap

### **Phase 1: ScyllaDB (Current) - 2-3 hours**

**Step 1: Schema** (30 min)
```bash
# Create tables
docker exec scylla cqlsh -f schema/active_orders.cql
docker exec scylla cqlsh -f schema/order_lookup.cql
```

**Step 2: ME Updates** (1 hour)
- Add `order_placements` to EngineOutput
- Add `order_updates` to EngineOutput
- Add `order_completions` to EngineOutput
- Update `place_order()` method
- Update `cancel_order()` method

**Step 3: Settlement Integration** (1 hour)
- Add `settle_engine_output_atomic()`
- Add partition key cache
- Add prepared statements
- Add batch processing

**Step 4: Gateway API** (30 min)
- Implement `get_active_orders()` handler
- Add response formatting
- Test E2E

---

### **Phase 2: ME Memory (Future) - 6-8 hours**

**Prerequisites:**
- QPS > 10K on active orders endpoint
- Latency SLA < 5ms required

**Implementation:**
- Add Aeron query message protocol
- Implement ME query handler
- Add request-response pattern
- Update Gateway to use Aeron query

**See:** `ACTIVE_ORDER_QUERY_DESIGN.md`

---

## ✅ Production Checklist

### **Phase 1 (ScyllaDB):**
- [ ] Schema deployed
- [ ] ME emits order lifecycle events
- [ ] Settlement processes atomically
- [ ] Partition key caching working
- [ ] Gateway API functional
- [ ] E2E test passing
- [ ] Monitoring added
- [ ] Documentation complete

### **Phase 2 (ME Memory) - When Needed:**
- [ ] Aeron query protocol defined
- [ ] ME query handler implemented
- [ ] Gateway uses Aeron for queries
- [ ] Fallback to DB on ME unavailable
- [ ] Load testing completed

---

## 📝 Key Design Principles

1. **Start Simple** - ScyllaDB first, optimize later
2. **Atomic Batches** - One EngineOutput = One batch
3. **Partition Key Caching** - Critical for performance
4. **Counter Updates** - Atomic, no race conditions
5. **Soft Delete** - Mark completed, cleanup later
6. **TTL** - Auto-expire old orders
7. **Idempotent** - IF NOT EXISTS for inserts
8. **Monitor** - Track all metrics

---

## 🎯 Success Metrics

**Phase 1:**
- Active orders query: <20ms (p99)
- Throughput: >1K QPS
- Data consistency: 100%
- Uptime: >99.9%

**Phase 2 (if implemented):**
- Active orders query: <2ms (p99)
- Throughput: >10K QPS
- ME query success rate: >99.9%

---

*Implementation Guide v1.0*
*Current: Phase 1 (ScyllaDB)*
*Future: Phase 2 (ME Memory, when needed)*
