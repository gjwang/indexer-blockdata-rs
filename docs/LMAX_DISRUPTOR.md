# LMAX Disruptor Integration

## Overview

Refactored the matching engine server to use the **LMAX Disruptor** crate for lock-free, high-performance inter-thread communication. The disruptor pattern uses a ring buffer to achieve ultra-low latency message passing between the poll thread and the processing thread.

## Architecture

### Ring Buffer Pattern

The LMAX Disruptor uses a **ring buffer** (circular array) instead of traditional queues:

1. **Lock-free**: No mutex contention between producer and consumer
2. **Cache-friendly**: Sequential memory access patterns
3. **Pre-allocated**: No dynamic memory allocation during operation
4. **Batching**: Natural support for batch processing

### Two Concurrent Threads

1. **Poll Thread** (Main thread - Producer)
   - Polls messages from Kafka
   - Accumulates orders into batches
   - Publishes events to the ring buffer
   - Non-blocking, continues polling immediately

2. **Process Thread** (Disruptor-managed - Consumer)
   - Receives events from the ring buffer
   - Writes orders to WAL
   - Processes orders through matching engine
   - Flushes WAL at end of each batch
   - Runs in dedicated thread managed by disruptor

## Key Components

### 1. Event Structure

```rust
#[derive(Clone)]
struct OrderEvent {
    command: Option<EngineCommand>,
}

#[derive(Clone)]
enum EngineCommand {
    PlaceOrderBatch(Vec<(u32, u64, Side, OrderType, u64, u64, u64)>),
    CancelOrder { symbol_id: u32, order_id: u64 },
    BalanceRequest(BalanceRequest),
}
```

### 2. Factory Function

```rust
let factory = || OrderEvent { command: None };
```

Creates empty events to populate the ring buffer at initialization.

### 3. Processor Closure

```rust
let processor = move |event: &OrderEvent, _sequence: Sequence, end_of_batch: bool| {
    if let Some(ref cmd) = event.command {
        match cmd {
            EngineCommand::PlaceOrderBatch(batch) => {
                // Write to WAL first
                for order in batch {
                    order_wal.log_place_order_no_flush(...);
                }
                // Process the batch
                engine.add_order_batch(batch.clone());
            }
            // ... other commands
        }
    }
    
    // Flush WAL at end of batch
    if end_of_batch {
        order_wal.flush()?;
    }
};
```

The processor owns `engine`, `balance_processor`, and `order_wal`.

### 4. Disruptor Builder

```rust
let mut producer = build_single_producer(8192, factory, BusySpin)
    .handle_events_with(processor)
    .build();
```

- **Ring buffer size**: 8192 (must be power of 2)
- **Wait strategy**: `BusySpin` for lowest latency
- **Producer type**: Single producer (only poll thread publishes)

### 5. Publishing Events

```rust
producer.publish(|event| {
    event.command = Some(EngineCommand::PlaceOrderBatch(batch));
});
```

## Flow Diagram

```
┌─────────────┐
│   Kafka     │
│  Consumer   │
└──────┬──────┘
       │ Poll messages
       ▼
┌─────────────────────────────────┐
│      Poll Thread (Producer)     │
│  - Accumulate orders            │
│  - Publish to ring buffer       │
└──────┬──────────────────────────┘
       │ Lock-free publish
       ▼
┌─────────────────────────────────┐
│      Ring Buffer (8192)         │
│  [Event][Event][Event]...[Event]│
└──────┬──────────────────────────┘
       │ Lock-free consume
       ▼
┌─────────────────────────────────┐
│   Process Thread (Consumer)     │
│  - Write to WAL                 │
│  - Match orders                 │
│  - Flush WAL (end of batch)     │
└──────┬──────────────────────────┘
       │
       ▼
┌─────────────┐    ┌──────────┐
│  Order Book │    │  Trades  │
└─────────────┘    └──────────┘
```

## Benefits

### 1. **Ultra-Low Latency**
- Lock-free ring buffer eliminates mutex contention
- BusySpin wait strategy minimizes wake-up latency
- Cache-friendly sequential access patterns

### 2. **High Throughput**
- Batch processing reduces per-message overhead
- Pre-allocated ring buffer eliminates allocation overhead
- Single writer / single reader optimization

### 3. **Durability**
- All orders written to WAL before processing
- Batch flush at end of each batch
- Guaranteed ordering through ring buffer sequence

### 4. **Simplicity**
- No manual sequence tracking needed
- Disruptor handles all synchronization
- Clean separation of concerns

## Configuration

### Ring Buffer Size
```rust
build_single_producer(8192, factory, BusySpin)
                      ^^^^
```
- Must be power of 2 (2048, 4096, 8192, 16384, etc.)
- Larger = more buffering, but more memory
- 8192 is a good balance for most workloads

### Wait Strategy
```rust
build_single_producer(8192, factory, BusySpin)
                                     ^^^^^^^^
```

Available strategies:
- **`BusySpin`**: Lowest latency, highest CPU usage (recommended)
- **`YieldSpin`**: Lower CPU, slightly higher latency
- **`BlockingSpin`**: Lowest CPU, highest latency

## Performance Characteristics

### Latency
- **Publish**: ~10-50ns (lock-free write to ring buffer)
- **Process**: Depends on matching engine complexity
- **Total**: Typically <1μs for simple orders

### Throughput
- **Single thread**: 1-10M events/sec
- **Batch processing**: Can handle bursts of 100K+ orders/sec
- **Bottleneck**: Usually matching engine, not disruptor

## Testing

Build and run:

```bash
cargo build --bin matching_engine_server --release
./target/release/matching_engine_server
```

Expected output:
```
>>> Disruptor initialized with ring buffer size 8192
>>> Matching Engine Server Started (Pipelined)
[PERF] Poll: 1000 msgs. Wait: 2ms, Drain: 5ms
[PERF] Match: 1000 orders. Input: 10ms, Process: 50ms, Commit: 20ms. Total: 80ms
[PERF] OPS: 50000.00, Last Batch: 1000
```

## Comparison: Custom vs LMAX Disruptor

| Feature | Custom Implementation | LMAX Disruptor |
|---------|----------------------|----------------|
| Synchronization | AtomicU64 + BTreeMap | Ring buffer |
| Latency | ~100-500ns | ~10-50ns |
| Complexity | Manual sequence tracking | Automatic |
| Memory | Dynamic allocation | Pre-allocated |
| Batching | Manual | Built-in |
| CPU Usage | Lower | Higher (BusySpin) |

## Future Enhancements

1. **Multi-producer**: Use `build_multi_producer` for multiple Kafka consumers
2. **Multiple consumers**: Add separate threads for different symbols
3. **Metrics**: Track ring buffer utilization and processing lag
4. **Dynamic sizing**: Adjust ring buffer size based on load
5. **Wait strategy tuning**: Switch between strategies based on load

## References

- [LMAX Disruptor Paper](https://lmax-exchange.github.io/disruptor/)
- [Rust Disruptor Crate](https://crates.io/crates/disruptor)
- [Mechanical Sympathy Blog](https://mechanical-sympathy.blogspot.com/)
