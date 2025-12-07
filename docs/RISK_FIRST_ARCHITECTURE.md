# Risk-First Architecture (Pre-Trade Risk Firewall)

## Industry Standard Pattern

This is the architecture used by world-class exchanges: **Binance, Coinbase, LMAX, Nasdaq**.

## The Fundamental Conflict

| Component | Optimal Sharding | Reason |
|-----------|------------------|--------|
| Matching Engine | By **SYMBOL** | BTC/USDT, ETH/USDT are independent order books |
| Balance State | By **USER** | User's USDT must be atomic across all symbols |

**Problem**: BTC/USDT and ETH/USDT share USDT balance. Parallel MEs = double-spend risk.

## Solution: Two-Step Routing

```
                         ┌─────────────────────────────┐
                         │        GATEWAY              │
                         │   (Stateless, Horizontal)   │
                         └─────────────┬───────────────┘
                                       │
                                       │ Route by user_id % N
                                       │
              ┌────────────────────────┼────────────────────────┐
              ▼                        ▼                        ▼
     ┌────────────────┐       ┌────────────────┐       ┌────────────────┐
     │  RISK ENGINE   │       │  RISK ENGINE   │       │  RISK ENGINE   │
     │    Shard 0     │       │    Shard 1     │       │    Shard 2     │
     │                │       │                │       │                │
     │ Users: 0,3,6.. │       │ Users: 1,4,7.. │       │ Users: 2,5,8.. │
     │                │       │                │       │                │
     │ ┌────────────┐ │       │ ┌────────────┐ │       │ ┌────────────┐ │
     │ │IN-MEMORY   │ │       │ │IN-MEMORY   │ │       │ │IN-MEMORY   │ │
     │ │HashMap<    │ │       │ │HashMap<    │ │       │ │HashMap<    │ │
     │ │(user,asset)│ │       │ │(user,asset)│ │       │ │(user,asset)│ │
     │ │, Balance>  │ │       │ │, Balance>  │ │       │ │, Balance>  │ │
     │ └────────────┘ │       │ └────────────┘ │       │ └────────────┘ │
     │                │       │                │       │                │
     │ SINGLE-THREAD  │       │ SINGLE-THREAD  │       │ SINGLE-THREAD  │
     │ (LMAX pattern) │       │ (LMAX pattern) │       │ (LMAX pattern) │
     └───────┬────────┘       └───────┬────────┘       └───────┬────────┘
             │                        │                        │
             │ If balance OK → Forward order with lock_version │
             │                        │                        │
             └────────────────────────┼────────────────────────┘
                                      │
                                      │ Route by symbol_id
                                      │
              ┌───────────────────────┼───────────────────────┐
              ▼                       ▼                       ▼
     ┌────────────────┐      ┌────────────────┐      ┌────────────────┐
     │ MATCHING ENGINE│      │ MATCHING ENGINE│      │ MATCHING ENGINE│
     │    BTC/USDT    │      │    ETH/USDT    │      │    XRP/USDT    │
     │                │      │                │      │                │
     │  NO BALANCE    │      │  NO BALANCE    │      │  NO BALANCE    │
     │  CHECK!        │      │  CHECK!        │      │  CHECK!        │
     │                │      │                │      │                │
     │  Trust Risk    │      │  Trust Risk    │      │  Trust Risk    │
     │  Engine 100%   │      │  Engine 100%   │      │  Engine 100%   │
     │                │      │                │      │                │
     │  Order Book    │      │  Order Book    │      │  Order Book    │
     │  Matching      │      │  Matching      │      │  Matching      │
     └───────┬────────┘      └───────┬────────┘      └───────┬────────┘
             │                       │                       │
             │                 TradeExecuted events          │
             │                       │                       │
             └───────────────────────┼───────────────────────┘
                                     │
                                     ▼
                         ┌───────────────────────┐
                         │  SETTLEMENT SERVICE   │
                         │                       │
                         │  - Unlock frozen      │
                         │  - Transfer assets    │
                         │  - Write to DB        │
                         │  - Update Risk Engine │
                         └───────────────────────┘
```

## Order Flow: Step by Step

### Normal Order
```
1. User 1001 submits: Buy 0.1 BTC @ 50,000 USDT

2. Gateway receives order
   └─→ Route to Risk Engine Shard (1001 % 3 = 2)

3. Risk Engine Shard 2:
   ├─ Check: user_1001.USDT.available >= 5000?
   ├─ If YES:
   │   ├─ user_1001.USDT.available -= 5000
   │   ├─ user_1001.USDT.locked += 5000
   │   ├─ Generate lock_id, increment balance_version
   │   └─ Forward to ME-BTC with {order, lock_id, version}
   └─ If NO:
       └─ Reject immediately (never reaches ME)

4. ME-BTC receives order:
   ├─ NO balance check (funds already locked)
   ├─ Add to order book
   ├─ Match against existing orders
   └─ Output: TradeExecuted(buyer=1001, seller=2002, qty=0.1, price=50000)

5. Settlement receives trade:
   ├─ Risk Shard 2: user_1001.USDT.locked -= 5000 (unlock)
   ├─ Risk Shard 2: user_1001.BTC.available += 0.1
   ├─ Risk Shard 0: user_2002.BTC.available -= 0.1
   ├─ Risk Shard 0: user_2002.USDT.available += 5000
   └─ Write to DB (async)
```

### Double-Spend Attempt
```
User 1001 has 5000 USDT
Attempts simultaneously:
  - Order A: Buy BTC with 5000 USDT
  - Order B: Buy ETH with 5000 USDT

1. Both orders hit Gateway at same nanosecond

2. Both routed to Risk Engine Shard 2 (user 1001 % 3 = 2)

3. Risk Engine Shard 2 is SINGLE-THREADED (LMAX Disruptor)
   Events processed one-by-one:

   Event A (arrives first):
   ├─ Check: available (5000) >= 5000? YES
   ├─ available = 5000 - 5000 = 0
   ├─ locked = 0 + 5000 = 5000
   └─ Forward to ME-BTC ✓

   Event B (arrives second):
   ├─ Check: available (0) >= 5000? NO
   └─ REJECT immediately ✗

4. ME-BTC receives Order A, matches it
5. ME-ETH never sees Order B

Result: No double-spend possible!
```

## Risk Engine Design

### Data Structure
```rust
struct RiskEngine {
    // Sharded by user_id
    shard_id: usize,

    // In-memory balance state (NO DATABASE QUERIES)
    balances: HashMap<(UserId, AssetId), Balance>,

    // Lock tracking
    locks: HashMap<LockId, LockInfo>,

    // Input queue (LMAX Disruptor / ring buffer)
    input_queue: RingBuffer<RiskEvent>,

    // Output channels to MEs
    me_channels: HashMap<SymbolId, Channel<Order>>,
}

struct Balance {
    available: u64,  // Can be used for new orders
    locked: u64,     // Reserved for pending orders
    version: u64,    // For optimistic concurrency
}

struct LockInfo {
    user_id: UserId,
    asset_id: AssetId,
    amount: u64,
    order_id: OrderId,
    created_at: u64,
}
```

### Core Loop (LMAX Pattern)
```rust
impl RiskEngine {
    // Single-threaded event loop - NO LOCKS NEEDED
    fn run(&mut self) {
        loop {
            // Busy-spin or wait on ring buffer
            if let Some(event) = self.input_queue.try_pop() {
                match event {
                    RiskEvent::PlaceOrder(order) => {
                        self.handle_place_order(order);
                    }
                    RiskEvent::CancelOrder(order_id) => {
                        self.handle_cancel(order_id);
                    }
                    RiskEvent::TradeSettlement(trade) => {
                        self.handle_settlement(trade);
                    }
                    RiskEvent::Deposit(user, asset, amount) => {
                        self.handle_deposit(user, asset, amount);
                    }
                }
            }
        }
    }

    fn handle_place_order(&mut self, order: Order) {
        let key = (order.user_id, order.lock_asset);
        let balance = self.balances.get_mut(&key);

        if balance.available >= order.lock_amount {
            // Lock funds
            balance.available -= order.lock_amount;
            balance.locked += order.lock_amount;
            balance.version += 1;

            // Record lock
            let lock_id = self.next_lock_id();
            self.locks.insert(lock_id, LockInfo { ... });

            // Forward to ME (async, non-blocking)
            let me_channel = &self.me_channels[&order.symbol_id];
            me_channel.send(OrderWithLock { order, lock_id, balance.version });
        } else {
            // Reject immediately
            self.reject_order(order, "Insufficient balance");
        }
    }
}
```

## Performance Characteristics

### Latency
| Step | Latency | Notes |
|------|---------|-------|
| Gateway → Risk Engine | 1-10 µs | Local IPC or fast network |
| Risk Engine check + lock | < 1 µs | Pure in-memory HashMap |
| Risk Engine → ME | 1-10 µs | Local IPC or fast network |
| ME matching | Variable | Depends on book depth |
| **Total pre-match** | **< 20 µs** | Compare to 100ms+ with DB |

### Throughput
| Component | Throughput | Notes |
|-----------|------------|-------|
| Risk Engine (single) | 1-10M events/sec | LMAX Disruptor pattern |
| Total (N shards) | N × 1-10M events/sec | Linear scaling |

### Memory
```
Per-user balance state: ~100 bytes
1M users × 10 assets = 1GB RAM
Easily fits in memory
```

## Persistence Strategy

### WAL (Write-Ahead Log)
```
Risk Engine does NOT wait for DB write before forwarding order.

Instead:
1. Lock balance in RAM
2. Write to local WAL (append-only, async)
3. Forward to ME immediately
4. Replicate WAL to backup node (async)
5. Periodic flush to DB (every N seconds)

Recovery:
1. Load from last DB snapshot
2. Replay WAL entries
3. Resume processing
```

### Replication
```
Risk-Shard-0 (Primary) ──async──> Risk-Shard-0' (Replica)

On primary failure:
1. Promote replica to primary
2. Replay any un-acked WAL entries
3. Resume (RTO < 1 second)
```

## Matching Engine Design (Simplified)

### No Balance Logic
```rust
struct MatchingEngine {
    symbol_id: SymbolId,
    order_book: OrderBook,

    // NO balance state!
    // NO balance checking!
}

impl MatchingEngine {
    fn handle_order(&mut self, order: OrderWithLock) {
        // Trust Risk Engine completely
        // If order arrived here, funds are already locked

        let trades = self.order_book.add_and_match(order.order);

        for trade in trades {
            self.output_channel.send(TradeExecuted {
                trade,
                buyer_lock_id: ...,
                seller_lock_id: ...,
            });
        }
    }
}
```

## Settlement Service

### Responsibilities
1. Receive TradeExecuted events from MEs
2. Update Risk Engine balance state (unlock + transfer)
3. Persist to database (async, batched)
4. Handle failures and retries

### Flow
```
TradeExecuted(buyer=1001, seller=2002, BTC=0.1, USDT=5000)
    │
    ├─→ Risk-Shard-2 (user 1001):
    │     - USDT.locked -= 5000
    │     - BTC.available += 0.1
    │
    ├─→ Risk-Shard-0 (user 2002):
    │     - BTC.locked -= 0.1
    │     - USDT.available += 5000
    │
    └─→ DB (async batch write)
```

## Migration Path

### Current State
```
Gateway → ME (with balance logic) → Settlement → DB
```

### Target State
```
Gateway → Risk Engine → ME (no balance) → Settlement → Risk + DB
```

### Phase 1: Extract Balance Logic
- Move balance HashMap from ME to separate RiskEngine struct
- Keep in same process initially
- Validate with tests

### Phase 2: Separate Process
- RiskEngine as separate service
- Gateway routes by user_id
- ME removes all balance logic

### Phase 3: Shard Risk Engines
- Multiple Risk Engine instances
- Load balancer routes by user_id
- Linear scaling

## Summary

| Principle | Implementation |
|-----------|----------------|
| Route by USER first | Risk Engine sharded by user_id |
| Lock before match | Risk Engine locks in RAM |
| Route by SYMBOL second | ME sharded by symbol_id |
| Trust the lock | ME does NO balance check |
| Async persistence | WAL + batch DB writes |
| Horizontal scaling | Add Risk shards + ME instances |
