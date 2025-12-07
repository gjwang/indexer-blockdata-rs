# Fan-In Serialization: Deterministic Settlement

## The Problem: Fan-In Concurrency

```
BTC-ME ──┐
         ├──> Settlement Service ──> Database
ETH-ME ──┘

Run 1: BTC arrives first → Balance: +BTC, -USDT, +ETH, -USDT
Run 2: ETH arrives first → Balance: +ETH, -USDT, +BTC, -USDT

Same inputs, same final balance, BUT different intermediate states!
This breaks deterministic replay.
```

**Network drift** causes non-deterministic ordering when parallel sources (MEs) feed into a single sink (Settlement).

## The Solution: The Serializer

**Don't have MEs call Settlement directly. Have them produce to Kafka/Redpanda.**

```
BTC-ME ──produce──┐
                  ├──> Redpanda Topic ──consume──> Settlement Service
ETH-ME ──produce──┘    (partitioned by user_id)

Redpanda assigns OFFSETS. Offset IS the deterministic order.
Replay = read from offset 0, same order every time.
```

## Architecture

```
┌─────────┐     ┌─────────┐     ┌─────────┐
│ BTC-ME  │     │ ETH-ME  │     │ XRP-ME  │
└────┬────┘     └────┬────┘     └────┬────┘
     │               │               │
     │    TradeExecuted events       │
     │               │               │
     └───────────────┼───────────────┘
                     │
                     ▼
     ┌───────────────────────────────────────┐
     │        REDPANDA / KAFKA               │
     │                                       │
     │  Topic: settlement_events             │
     │  Partitioned by: user_id % N          │
     │                                       │
     │  ┌─────────────────────────────────┐  │
     │  │ Partition 0 (user_id % 3 == 0)  │  │
     │  │ Offset 100: ETH trade (user 3)  │  │
     │  │ Offset 101: BTC trade (user 6)  │  │
     │  │ Offset 102: BTC trade (user 9)  │  │
     │  └─────────────────────────────────┘  │
     │                                       │
     │  ┌─────────────────────────────────┐  │
     │  │ Partition 1 (user_id % 3 == 1)  │  │
     │  │ Offset 100: BTC trade (user 1)  │  │
     │  │ Offset 101: ETH trade (user 4)  │  │
     │  └─────────────────────────────────┘  │
     └───────────────────┬───────────────────┘
                         │
            Consumer group: settlement_service
                         │
     ┌───────────────────┼───────────────────┐
     ▼                   ▼                   ▼
┌─────────────┐   ┌─────────────┐   ┌─────────────┐
│ Settlement  │   │ Settlement  │   │ Settlement  │
│  Worker 0   │   │  Worker 1   │   │  Worker 2   │
│(partition 0)│   │(partition 1)│   │(partition 2)│
└─────────────┘   └─────────────┘   └─────────────┘
```

## Key Properties

| Property | Guarantee |
|----------|-----------|
| **Ordering per user** | Kafka partition = total order |
| **Parallelism** | Different users on different partitions |
| **Durability** | Kafka persists events |
| **Replay** | Read from offset 0, same order every time |
| **Exactly-once** | Consumer commits offset after processing |

## The Event Struct

```rust
/// Event produced by Matching Engine to Kafka
#[derive(Serialize, Deserialize)]
struct SettlementEvent {
    // === Ordering (Assigned by Kafka) ===
    // log_offset: u64,  // This comes from Kafka, not the event itself

    // === Causality Chain ===
    user_id: u64,              // Partition key
    risk_seq_id: u64,          // Sequence from Risk Engine (input order)
    me_seq_id: u64,            // Sequence from Matching Engine
    symbol_id: u32,            // Which ME produced this

    // === Trade Data ===
    trade_id: u64,
    buyer_user_id: u64,
    seller_user_id: u64,
    base_asset_id: u32,
    quote_asset_id: u32,
    quantity: u64,
    price: u64,

    // === Balance Changes (Pre-computed by ME) ===
    buyer_base_delta: i64,     // +0.1 BTC
    buyer_quote_delta: i64,    // -5000 USDT
    seller_base_delta: i64,    // -0.1 BTC
    seller_quote_delta: i64,   // +5000 USDT
    buyer_fee: u64,
    seller_fee: u64,

    // === Timestamps (for debugging, NOT ordering) ===
    me_timestamp_ns: u64,
}
```

## Flow: Live Processing

```
1. BTC-ME matches trade: buyer=1001, seller=2002

2. BTC-ME produces to Kafka:
   Topic: settlement_events
   Partition: hash(1001) % 3 = 2  (buyer's partition)
   Value: SettlementEvent { buyer_user_id: 1001, ... }

   Also produces second event for seller:
   Partition: hash(2002) % 3 = 0  (seller's partition)
   Value: SettlementEvent { seller_user_id: 2002, ... }

3. Kafka assigns offsets:
   Partition 2, Offset 456 → buyer side
   Partition 0, Offset 789 → seller side

4. Settlement workers consume:
   Worker 2 reads partition 2, offset 456
   Worker 0 reads partition 0, offset 789

5. Workers update Risk Engine + DB
   Then commit offsets to Kafka
```

## Flow: Replay (Crash Recovery)

```
1. Settlement Worker 2 restarts

2. Check state: "Last committed offset: 455"

3. Seek Kafka partition 2, offset 456

4. Process offset 456 (buyer balance update)
   - Update Risk Engine
   - Write to DB
   - Commit offset 456

5. Process offset 457, 458, ...

6. Caught up! Resume normal processing

Result: IDENTICAL state to before crash
```

## Ordering Guarantees

### Within Same User
```
User 1001 places two orders:
- Order A: BTC trade
- Order B: ETH trade

Both settlement events go to partition (1001 % 3 = 2)
Kafka guarantees FIFO within partition
→ Settlement sees them in order

Even if ETH-ME produces before BTC-ME (network drift),
the events are serialized in Kafka.
```

### Cross-User
```
User 1001 (partition 2) and User 2002 (partition 0)
Their events are on DIFFERENT partitions.
No ordering guarantee needed! They're independent.
```

## Settlement Service Implementation

```rust
#[tokio::main]
async fn main() {
    let consumer = create_kafka_consumer("settlement_events");
    let risk_client = RiskEngineClient::new();
    let db = SettlementDb::new();

    loop {
        // Consume batch from Kafka
        let events = consumer.poll(Duration::from_millis(100)).await;

        for event in events {
            let offset = event.offset();
            let data: SettlementEvent = deserialize(event.value());

            // Process (idempotent - use offset as dedup key)
            process_settlement(&risk_client, &db, &data, offset).await;

            // Commit AFTER processing
            consumer.commit(offset).await;
        }
    }
}

async fn process_settlement(
    risk: &RiskEngineClient,
    db: &SettlementDb,
    event: &SettlementEvent,
    offset: u64,
) {
    // Idempotency check (have we processed this offset before?)
    if db.is_offset_processed(offset).await {
        return; // Already done, skip
    }

    // Update Risk Engine (in-memory balances)
    risk.apply_trade(
        event.user_id,
        event.base_asset_id, event.buyer_base_delta,
        event.quote_asset_id, event.buyer_quote_delta,
    ).await;

    // Persist to DB (can be batched for throughput)
    db.write_balance_change(event, offset).await;

    // Mark offset as processed
    db.mark_offset_processed(offset).await;
}
```

## Comparison: Current vs Target

### Current (ZMQ Push/Pull)
```
ME ──ZMQ PUSH──> Settlement ──> DB

Problems:
- ZMQ doesn't persist events
- No replay capability
- Non-deterministic if multiple MEs
- Lost events on Settlement crash
```

### Target (Kafka Serializer)
```
ME ──Kafka Produce──> Kafka Topic ──Consume──> Settlement ──> DB

Benefits:
- Kafka persists events
- Replay from offset 0
- Deterministic (Kafka assigns order)
- No lost events (Kafka retains)
- Consumer group for parallel processing
```

## Golden Rules

1. **Execution is Non-Deterministic**: ME timing varies
2. **Settlement is Deterministic**: Based on Kafka offsets
3. **Log is the Truth**: Not wall-clock, not arrival time
4. **Ignore timestamps for ordering**: Use Kafka offset only
5. **Idempotency**: Handle reprocessing gracefully

## Migration Path

### Step 1: Add Kafka Producer to ME
```rust
// In Matching Engine, after trade execution:
let event = SettlementEvent { ... };
kafka_producer.send(
    topic: "settlement_events",
    key: user_id.to_bytes(),  // Partition key
    value: serialize(event),
).await;
```

### Step 2: Convert Settlement to Kafka Consumer
```rust
// Replace ZMQ subscriber with Kafka consumer
// Process events by offset order
// Commit after processing
```

### Step 3: Remove ZMQ Pipeline
Once Kafka path is validated, remove the old ZMQ path.

### Step 4: Add Risk Engine Updates
Settlement now updates Risk Engine (in-memory) + DB (persistent).
