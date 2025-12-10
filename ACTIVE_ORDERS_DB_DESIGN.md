# Active Orders - ScyllaDB Query (Simple Approach)

**Date:** 2025-12-10
**Status:** 🎯 Simplified Design

---

## 🎯 Approach: Query from ScyllaDB

**Key Insight:** Orders should already be in ScyllaDB for:
- Audit trail
- Recovery
- Historical queries

So we can query active orders directly from DB!

---

## 📊 Database Schema

### **Table: `active_orders`**

```sql
CREATE TABLE IF NOT EXISTS active_orders (
    user_id bigint,
    symbol_id int,
    order_id bigint,

    -- Order details
    side tinyint,          -- 0=Buy, 1=Sell
    order_type tinyint,    -- 0=Market, 1=Limit
    price bigint,          -- Price in satoshis
    quantity bigint,       -- Total quantity
    filled_qty bigint,     -- Filled so far

    -- Status
    status tinyint,        -- 0=New, 1=PartialFill, 2=Filled, 3=Cancelled

    -- Timestamps
    created_at bigint,
    updated_at bigint,

    PRIMARY KEY ((user_id, symbol_id), created_at, order_id)
) WITH CLUSTERING ORDER BY (created_at DESC, order_id DESC)
  AND comment = 'Active orders by user and symbol';

-- Index for order_id lookup
CREATE INDEX IF NOT EXISTS active_orders_by_order_id
ON active_orders (order_id);
```

**Why this schema works:**
- Partition key: `(user_id, symbol_id)` - Perfect for API query!
- Clustering: `created_at DESC` - Newest first
- Can query all active orders for a user+symbol in O(1)

---

## 🔄 Order Lifecycle

### **1. Order Placed** (UBSCore)

```rust
// UBSCore: After validating order
async fn on_order_placed(
    db: &SettlementDb,
    order: &InternalOrder,
) -> Result<()> {
    db.insert_active_order(ActiveOrderInsert {
        user_id: order.user_id,
        symbol_id: get_symbol_id(order.symbol),
        order_id: order.order_id,
        side: order.side,
        order_type: order.order_type,
        price: order.price,
        quantity: order.quantity,
        filled_qty: 0,
        status: OrderStatus::New,
        created_at: now(),
        updated_at: now(),
    }).await
}
```

### **2. Order Partially Filled** (Settlement Service)

```rust
// Settlement: After trade settles
async fn on_trade_settled(
    db: &SettlementDb,
    trade: &TradeOutput,
) -> Result<()> {
    // Update buyer's order
    db.update_order_fill(
        trade.buy_order_id,
        trade.quantity,
    ).await?;

    // Update seller's order
    db.update_order_fill(
        trade.sell_order_id,
        trade.quantity,
    ).await?;

    Ok(())
}

// Update filled quantity
async fn update_order_fill(
    &self,
    order_id: u64,
    fill_qty: u64,
) -> Result<()> {
    // Get current order
    let order = self.get_order_by_id(order_id).await?;
    let new_filled = order.filled_qty + fill_qty;

    // Determine new status
    let status = if new_filled >= order.quantity {
        OrderStatus::Filled  // Fully filled - will be removed
    } else {
        OrderStatus::PartialFill
    };

    // Update order
    self.session.query(
        "UPDATE active_orders
         SET filled_qty = ?, status = ?, updated_at = ?
         WHERE user_id = ? AND symbol_id = ?
           AND created_at = ? AND order_id = ?",
        (new_filled, status as i8, now(),
         order.user_id, order.symbol_id, order.created_at, order_id)
    ).await?;

    // If fully filled, delete from active_orders
    if status == OrderStatus::Filled {
        self.delete_active_order(order_id).await?;
    }

    Ok(())
}
```

### **3. Order Cancelled** (Settlement Service)

```rust
// Settlement: After cancel processed
async fn on_order_cancelled(
    db: &SettlementDb,
    order_id: u64,
) -> Result<()> {
    // Delete from active_orders
    db.delete_active_order(order_id).await
}

async fn delete_active_order(&self, order_id: u64) -> Result<()> {
    // Get order details first (need partition key)
    let order = self.get_order_by_id(order_id).await?;

    self.session.query(
        "DELETE FROM active_orders
         WHERE user_id = ? AND symbol_id = ?
           AND created_at = ? AND order_id = ?",
        (order.user_id, order.symbol_id, order.created_at, order_id)
    ).await?;

    Ok(())
}
```

---

## 📖 Gateway Query (SIMPLE!)

### **Implementation:**

```rust
// File: src/gateway.rs
async fn get_active_orders(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<ActiveOrderResponse>>>, (StatusCode, String)> {
    let user_id = params.user_id;

    // Get symbol_id
    let symbol_id = match state.symbol_manager.get_symbol_id(&params.symbol) {
        Some(id) => id,
        None => return Ok(Json(ApiResponse::success(vec![]))),
    };

    if let Some(db) = &state.db {
        // Query active orders from DB - SIMPLE!
        let active_orders = db
            .get_active_orders(user_id, symbol_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Convert to response
        let response: Vec<ActiveOrderResponse> = active_orders
            .into_iter()
            .map(|o| {
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
            .filter_map(|x| x)
            .collect();

        Ok(Json(ApiResponse::success(response)))
    } else {
        Ok(Json(ApiResponse::success(vec![])))
    }
}
```

### **DB Function:**

```rust
// File: src/db/settlement_db.rs
pub async fn get_active_orders(
    &self,
    user_id: u64,
    symbol_id: u32,
) -> Result<Vec<ActiveOrder>> {
    let result = self.session.query(
        "SELECT * FROM active_orders
         WHERE user_id = ? AND symbol_id = ?
         AND status IN (0, 1)",  // Only New and PartialFill
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
```

---

## ✅ Benefits of DB Approach

1. **Simplicity** ✅
   - No Aeron request-response needed
   - No ME query handler needed
   - Just DB query!

2. **Consistency** ✅
   - Same pattern as trade history
   - Uses existing DB infrastructure
   - Already have recovery data

3. **Fast Implementation** ✅
   - 2-3 hours vs 6-8 hours
   - Less complexity
   - Fewer components

4. **Audit Trail** ✅
   - Orders persisted
   - Can query historical state
   - Recovery friendly

---

## ⚠️ Trade-offs

**Latency:**
- Aeron: <1ms
- ScyllaDB: 5-10ms
- **Still acceptable!**

**Freshness:**
- Slight delay (ms) between ME and DB
- **Acceptable for most use cases**

**Write Load:**
- Every order create/update writes to DB
- **Already writing anyway for audit!**

---

## 🚀 Implementation Steps

### **Step 1: Create Schema** (15 min)
```sql
-- Create active_orders table
-- Create index on order_id
```

### **Step 2: Add DB Functions** (30 min)
```rust
- insert_active_order()
- update_order_fill()
- delete_active_order()
- get_active_orders()
```

### **Step 3: Integration** (1 hour)
```rust
// UBSCore: Write on order create
// Settlement: Update on fill/cancel
```

### **Step 4: Gateway** (30 min)
```rust
// Update get_active_orders() to query DB
```

### **Step 5: Test** (30 min)
```
// E2E test
```

**Total: 2.5-3 hours** ✅

---

## 📊 Comparison

| Aspect | Aeron Query | ScyllaDB Query |
|--------|-------------|----------------|
| **Latency** | <1ms | 5-10ms |
| **Complexity** | High | Low |
| **Implementation** | 6-8h | 2-3h |
| **Dependencies** | ME changes | None (DB exists) |
| **Audit** | Need separate | Built-in |
| **Recovery** | Need WAL | Built-in |
| **Recommendation** | Later | **NOW** ✅ |

---

## 🎯 Recommendation

**START WITH SCYLLADB:**
1. ✅ Simpler implementation
2. ✅ Faster to deliver
3. ✅ Provides audit trail
4. ✅ Good enough performance (5-10ms)

**OPTIMIZE LATER IF NEEDED:**
- If latency becomes issue
- If QPS > 10K
- Add ME direct query then

**This is the pragmatic choice!** 🚀

---

*Updated: 2025-12-10*
*Recommended Approach: ScyllaDB Query*
