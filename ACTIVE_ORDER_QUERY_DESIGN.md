# Active Order Query Architecture

**Date:** 2025-12-10
**Status:** 🎯 Design Document

---

## 🎯 Overview

Active orders are orders currently open in the Matching Engine that haven't been filled or cancelled. They exist in ME memory (OrderBook), not in the database.

---

## 🏗️ Architecture Options

### **Option 1: Direct ME Query (Recommended)**

**Flow:**
```
Gateway → Aeron Request → ME → Aeron Response → Gateway
```

**Components:**

1. **Gateway sends query via Aeron**
   ```rust
   // Gateway: src/ubs_core/comm/gateway_client.rs
   pub async fn query_active_orders(
       &self,
       user_id: u64,
       symbol_id: u32,
   ) -> Result<Vec<ActiveOrder>> {
       let query = QueryMessage {
           msg_type: MsgType::Query,
           user_id,
           symbol_id,
           query_type: QueryType::ActiveOrders,
       };

       // Send via Aeron, wait for response
       self.send_with_response(query, 1000).await
   }
   ```

2. **ME handles query**
   ```rust
   // ME: src/bin/matching_engine_server.rs
   fn handle_query_message(
       engine: &MatchingEngine,
       query: QueryMessage,
   ) -> QueryResponse {
       match query.query_type {
           QueryType::ActiveOrders => {
               let orders = engine.get_active_orders(
                   query.user_id,
                   query.symbol_id
               );
               QueryResponse::ActiveOrders(orders)
           }
       }
   }
   ```

3. **ME returns active orders**
   ```rust
   // ME: src/matching_engine_base.rs
   pub fn get_active_orders(
       &self,
       user_id: u64,
       symbol_id: u32,
   ) -> Vec<ActiveOrderInfo> {
       let book = match self.order_books.get(&symbol_id) {
           Some(b) => b,
           None => return vec![],
       };

       book.get_user_orders(user_id)
   }
   ```

**Pros:**
- ✅ Fast (in-memory query)
- ✅ Uses existing Aeron infrastructure
- ✅ Real-time data from ME
- ✅ No database needed

**Cons:**
- ⚠️ Requires Aeron request-response pattern
- ⚠️ ME needs query handler

---

### **Option 2: HTTP Query Endpoint**

**Flow:**
```
Gateway → HTTP GET → ME HTTP API → JSON Response → Gateway
```

**Implementation:**
```rust
// ME: Add HTTP server
let query_api = Router::new()
    .route("/active_orders", get(get_active_orders));

async fn get_active_orders(
    Query(params): Query<ActiveOrderQuery>,
) -> Json<Vec<ActiveOrder>> {
    // Query ME state
    let orders = ENGINE_STATE.get_active_orders(
        params.user_id,
        params.symbol_id
    );
    Json(orders)
}
```

**Pros:**
- ✅ Simple HTTP protocol
- ✅ Easy to debug/test
- ✅ Can use curl for testing

**Cons:**
- ⚠️ Additional HTTP server in ME
- ⚠️ Slower than Aeron
- ⚠️ Network overhead

---

### **Option 3: Snapshot to Database**

**Flow:**
```
ME → Periodic snapshot → DB → Gateway queries DB
```

**Implementation:**
```rust
// ME: Periodic snapshot (every 1s)
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        snapshot_active_orders(&engine, &db).await;
    }
});

async fn snapshot_active_orders(
    engine: &MatchingEngine,
    db: &Database,
) {
    for (symbol_id, book) in &engine.order_books {
        let active_orders = book.get_all_active_orders();
        db.upsert_active_orders(symbol_id, active_orders).await;
    }
}
```

**Pros:**
- ✅ Gateway uses simple DB query
- ✅ No direct ME coupling
- ✅ Can query historical snapshots

**Cons:**
- ⚠️ Stale data (up to 1s delay)
- ⚠️ High write load on DB
- ⚠️ Complex state management

---

## 🎯 Recommended Design: Option 1 (Aeron Query)

### **Why Aeron:**
1. Already used for order submission
2. Lowest latency (<1ms)
3. Keeps all order logic in ME
4. No additional infrastructure

---

## 📊 Detailed Implementation

### **1. Define Message Types**

**File:** `src/ubs_core/comm/message.rs`

```rust
#[repr(u8)]
pub enum MsgType {
    Order = 1,
    Cancel = 2,
    Query = 3,           // ← Add this
    Deposit = 4,
    Withdraw = 5,
    Response = 128,      // ← Add this
}

#[repr(u8)]
pub enum QueryType {
    ActiveOrders = 1,
    OrderStatus = 2,
}

// Query message format:
// [MsgType::Query (1 byte)]
// [QueryType (1 byte)]
// [user_id (8 bytes)]
// [symbol_id (4 bytes)]
```

### **2. Add OrderBook Query Method**

**File:** `src/matching_engine_base.rs`

```rust
impl OrderBook {
    /// Get all active orders for a specific user
    pub fn get_user_orders(&self, user_id: u64) -> Vec<ActiveOrderInfo> {
        let mut orders = Vec::new();

        // Check buy side
        for order in &self.buy_orders {
            if order.user_id == user_id {
                orders.push(ActiveOrderInfo {
                    order_id: order.order_id,
                    user_id: order.user_id,
                    side: Side::Buy,
                    order_type: order.order_type,
                    price: order.price,
                    quantity: order.quantity,
                    filled_quantity: order.filled_quantity,
                    status: calculate_status(order),
                    created_at: order.created_at,
                });
            }
        }

        // Check sell side
        for order in &self.sell_orders {
            if order.user_id == user_id {
                orders.push(ActiveOrderInfo {
                    order_id: order.order_id,
                    user_id: order.user_id,
                    side: Side::Sell,
                    order_type: order.order_type,
                    price: order.price,
                    quantity: order.quantity,
                    filled_quantity: order.filled_quantity,
                    status: calculate_status(order),
                    created_at: order.created_at,
                });
            }
        }

        orders
    }
}

/// Active order information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveOrderInfo {
    pub order_id: u64,
    pub user_id: u64,
    pub side: Side,
    pub order_type: OrderType,
    pub price: u64,
    pub quantity: u64,
    pub filled_quantity: u64,
    pub status: OrderStatus,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderStatus {
    New,           // Just placed, not filled
    PartialFill,   // Partially filled
    Open,          // Active in order book
}

fn calculate_status(order: &Order) -> OrderStatus {
    if order.filled_quantity == 0 {
        OrderStatus::New
    } else if order.filled_quantity < order.quantity {
        OrderStatus::PartialFill
    } else {
        OrderStatus::Open
    }
}
```

### **3. ME Query Handler**

**File:** `src/bin/matching_engine_server.rs`

```rust
// Add query handler to ME
async fn handle_query_request(
    engine: Arc<Mutex<MatchingEngine>>,
    user_id: u64,
    symbol_id: u32,
    query_type: QueryType,
) -> Vec<ActiveOrderInfo> {
    let engine = engine.lock().unwrap();

    match query_type {
        QueryType::ActiveOrders => {
            engine.get_active_orders(user_id, symbol_id)
        }
        QueryType::OrderStatus => {
            // Future: query specific order status
            vec![]
        }
    }
}
```

### **4. Gateway Integration**

**File:** `src/gateway.rs`

```rust
async fn get_active_orders(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<ActiveOrderResponse>>>, (StatusCode, String)> {
    let user_id = params.user_id;
    let symbol_id = match state.symbol_manager.get_symbol_id(&params.symbol) {
        Some(id) => id,
        None => return Ok(Json(ApiResponse::success(vec![]))),
    };

    #[cfg(feature = "aeron")]
    {
        // Query ME via Aeron
        let active_orders = state
            .ubs_client
            .query_active_orders(user_id, symbol_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Convert to response format
        let response: Vec<ActiveOrderResponse> = active_orders
            .into_iter()
            .map(|o| ActiveOrderResponse {
                order_id: o.order_id.to_string(),
                symbol: params.symbol.clone(),
                side: format!("{:?}", o.side).to_uppercase(),
                order_type: format!("{:?}", o.order_type),
                price: state.balance_manager.to_client_price(&params.symbol, o.price).unwrap(),
                quantity: state.balance_manager.to_client_amount(
                    state.symbol_manager.get_base_asset(&params.symbol).unwrap(),
                    o.quantity
                ).unwrap(),
                filled_quantity: state.balance_manager.to_client_amount(
                    state.symbol_manager.get_base_asset(&params.symbol).unwrap(),
                    o.filled_quantity
                ).unwrap(),
                status: format!("{:?}", o.status),
                created_at: o.created_at,
            })
            .collect();

        Ok(Json(ApiResponse::success(response)))
    }

    #[cfg(not(feature = "aeron"))]
    {
        Err((StatusCode::SERVICE_UNAVAILABLE, "Aeron not enabled".to_string()))
    }
}

#[derive(Debug, Serialize)]
struct ActiveOrderResponse {
    order_id: String,
    symbol: String,
    side: String,
    order_type: String,
    price: Decimal,
    quantity: Decimal,
    filled_quantity: Decimal,
    status: String,
    created_at: u64,
}
```

---

## 🔄 Request-Response Pattern

### **Aeron Request-Response Implementation**

```rust
// Gateway sends query
pub async fn query_active_orders(
    &self,
    user_id: u64,
    symbol_id: u32,
) -> Result<Vec<ActiveOrderInfo>> {
    // Build query message
    let mut payload = Vec::with_capacity(14);
    payload.push(MsgType::Query as u8);
    payload.push(QueryType::ActiveOrders as u8);
    payload.extend_from_slice(&user_id.to_le_bytes());
    payload.extend_from_slice(&symbol_id.to_le_bytes());

    // Send and await response
    let response = self.send_with_response(&payload, timeout_ms).await?;

    // Parse response
    bincode::deserialize(&response)
        .map_err(|e| format!("Failed to parse response: {}", e))
}

// ME processes query and sends response
fn process_query(payload: &[u8], engine: &MatchingEngine) -> Vec<u8> {
    let query_type = QueryType::from(payload[1]);
    let user_id = u64::from_le_bytes(payload[2..10].try_into().unwrap());
    let symbol_id = u32::from_le_bytes(payload[10..14].try_into().unwrap());

    let orders = engine.get_active_orders(user_id, symbol_id);

    // Serialize response
    bincode::serialize(&orders).unwrap()
}
```

---

## 📊 Performance Characteristics

| Metric | Aeron Query | HTTP Query | DB Snapshot |
|--------|-------------|------------|-------------|
| **Latency** | <1ms | 5-10ms | 10-50ms |
| **Freshness** | Real-time | Real-time | 1s stale |
| **Throughput** | 100K+ QPS | 10K QPS | Unlimited (DB) |
| **Complexity** | Medium | Low | High |
| **Dependencies** | Aeron | HTTP server | Database |

---

## 🎯 Implementation Phases

### **Phase 1: Core Functionality (2-3 hours)**
1. Add query message types
2. Implement `OrderBook::get_user_orders()`
3. Add ME query handler
4. Test with direct ME calls

### **Phase 2: Aeron Integration (2-3 hours)**
5. Implement request-response in UbsGatewayClient
6. Add ME Aeron response sender
7. Test Gateway → ME → Gateway flow

### **Phase 3: Gateway API (1 hour)**
8. Update `get_active_orders()` handler
9. Add response conversion
10. Test E2E via API

### **Phase 4: Testing & Polish (1 hour)**
11. Add E2E test
12. Performance testing
13. Documentation

**Total Estimated Time:** 6-8 hours

---

## ✅ Acceptance Criteria

1. **Functional:**
   - ✅ Gateway can query active orders from ME
   - ✅ Returns correct order data
   - ✅ Handles empty results gracefully
   - ✅ Error handling works

2. **Performance:**
   - ✅ Query latency <10ms (p99)
   - ✅ Can handle 1000+ QPS
   - ✅ No memory leaks

3. **Quality:**
   - ✅ Code compiles
   - ✅ Tests pass
   - ✅ Documented

---

## 🔐 Security Considerations

1. **Authorization:**
   - Verify user_id matches authenticated user
   - Don't leak other users' orders

2. **Rate Limiting:**
   - Limit queries per user (e.g., 10/sec)
   - Prevent ME overload

3. **Input Validation:**
   - Validate symbol_id exists
   - Validate user_id format

---

## 📝 API Contract

### **Request:**
```
GET /api/v1/order/active?user_id=1001&symbol=BTC_USDT
```

### **Response:**
```json
{
  "status": 0,
  "msg": "ok",
  "data": [
    {
      "order_id": "1234567890",
      "symbol": "BTC_USDT",
      "side": "BUY",
      "order_type": "Limit",
      "price": "50000.00",
      "quantity": "0.1",
      "filled_quantity": "0.05",
      "status": "PartialFill",
      "created_at": 1234567890000
    }
  ]
}
```

---

## 🚀 Next Steps

1. **Review this design** ✅
2. **Approve architecture**
3. **Implement Phase 1** (Core)
4. **Implement Phase 2** (Aeron)
5. **Implement Phase 3** (Gateway)
6. **Test & Deploy**

---

*Design Complete: 2025-12-10*
*Ready for Implementation*
