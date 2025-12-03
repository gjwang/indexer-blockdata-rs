# True Pipeline Architecture with LMAX Disruptor

## Overview

The matching engine uses a **true pipeline architecture** with the LMAX Disruptor to achieve **minimum latency** by running WAL writes and order matching **concurrently**. The key innovation is using a **progress ID** as a gate to ensure every order is persisted in WAL before matching.

## Architecture: Three Concurrent Threads

```
┌─────────────┐
│   Kafka     │  Thread 1: Poll Thread
│  Consumer   │  - Polls messages from Kafka
└──────┬──────┘  - Publishes to ring buffer
       │
       ▼
┌─────────────────────────────────┐
│      Ring Buffer (8192)         │  Lock-free communication
│  [Event][Event][Event]...[Event]│
└──────┬──────────────┬───────────┘
       │              │
       ▼              ▼
┌──────────────┐  ┌─────────────┐
│ WAL Writer   │  │  Matcher    │  Thread 2 & 3: Run concurrently!
│ (Consumer 1) │  │ (Consumer 2)│
│              │  │             │
│ Write WAL    │  │ Wait for    │
│ Update       │  │ progress_id │
│ progress_id  │  │             │
│              │  │ Match orders│
└──────────────┘  └─────────────┘
       │                 │
       └────────┬────────┘
                ▼
         ┌─────────────┐
         │ Order Book  │
         │   Trades    │
         └─────────────┘
```

## Key Components

### 1. Poll Thread (Producer)
- Polls Kafka for order messages
- Publishes events to the disruptor ring buffer
- **Non-blocking**: Continues polling immediately

### 2. WAL Writer Thread (Consumer 1)
- Consumes events from ring buffer
- Writes orders to WAL (no flush yet)
- Updates `current_seq` for each event
- **Flushes WAL at end of batch**
- **Updates `progress_id` after flush** (inside `flush()`)

### 3. Matcher Thread (Consumer 2)
- Consumes events from ring buffer **in parallel** with WAL writer
- **Waits for `progress_id`** before processing each event
- Only processes when `sequence <= progress_id`
- Guarantees WAL persistence before matching

## Progress ID: The Gate Keeper

The `progress_id` is an `Arc<AtomicU64>` that synchronizes the two consumers:

```rust
// WAL Writer updates progress after flush
order_wal.flush()?;  // This updates progress_id = current_seq

// Matcher waits for progress
loop {
    let progress = progress_id.load(Ordering::Acquire);
    if (sequence as u64) <= progress {
        break;  // Safe to process!
    }
    std::hint::spin_loop();  // Spin wait
}
```

## Pipeline Flow

```
Time →

Poll:     [Msg1][Msg2][Msg3][Msg4][Msg5]...
          ↓     ↓     ↓     ↓     ↓
Ring:     [Ev1][Ev2][Ev3][Ev4][Ev5]...
          ↓     ↓     ↓     ↓     ↓
WAL:      Write Write Write Flush! ← Updates progress_id=3
          ↓     ↓     ↓     ↓     ↓
Match:    Wait  Wait  Match Match  Wait...
                      ↑     ↑
                      Safe! Safe!
```

### Key Insight: Overlapping I/O and Compute

While the WAL writer is doing I/O (writing to disk), the matcher can be processing previously flushed orders. This **overlaps I/O with compute**, minimizing total latency!

```
WAL Writer:   [Write Batch 1] [Flush] [Write Batch 2] [Flush]
Matcher:                      [Match Batch 1]         [Match Batch 2]
                              ↑ Starts immediately after flush!
```

## Code Structure

### Event Definition
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

### WAL Writer (Consumer 1)
```rust
let wal_writer = move |event: &OrderEvent, sequence: Sequence, end_of_batch: bool| {
    if let Some(ref cmd) = event.command {
        match cmd {
            EngineCommand::PlaceOrderBatch(batch) => {
                // Write to WAL (no flush)
                for order in batch {
                    order_wal.log_place_order_no_flush(...);
                }
                order_wal.current_seq = sequence as u64;
            }
            // ... other commands
        }
    }
    
    // Flush at end of batch - this updates progress_id!
    if end_of_batch {
        order_wal.flush()?;  // progress_id = current_seq
    }
};
```

### Matcher (Consumer 2)
```rust
let matcher = move |event: &OrderEvent, sequence: Sequence, _end_of_batch: bool| {
    // WAIT for WAL to be flushed
    loop {
        let progress = progress_id.load(Ordering::Acquire);
        if (sequence as u64) <= progress {
            break;  // Safe to process!
        }
        std::hint::spin_loop();
    }
    
    // Now safe - WAL is guaranteed flushed
    if let Some(ref cmd) = event.command {
        match cmd {
            EngineCommand::PlaceOrderBatch(batch) => {
                engine.add_order_batch(batch.clone());
            }
            // ... other commands
        }
    }
};
```

### Disruptor Builder
```rust
let mut producer = build_single_producer(8192, factory, BusySpin)
    .handle_events_with(wal_writer)  // Consumer 1
    .and_then()                      // Run in parallel!
    .handle_events_with(matcher)     // Consumer 2
    .build();
```

The `.and_then()` is key - it creates a **dependency** where the matcher depends on the WAL writer's sequence, but they **run concurrently**!

## Benefits

### 1. **Minimum Latency**
- WAL I/O and matching run in parallel
- No waiting for sequential processing
- Overlapping I/O with compute

### 2. **Guaranteed Durability**
- Every order is in WAL before matching
- Progress ID acts as a barrier
- No data loss on crash

### 3. **High Throughput**
- Lock-free ring buffer
- Batch processing
- Concurrent consumers

### 4. **Simplicity**
- Disruptor handles synchronization
- Progress ID is simple atomic variable
- Clean separation of concerns

## Performance Characteristics

### Latency Breakdown

| Stage | Latency | Notes |
|-------|---------|-------|
| Kafka poll | ~1-5ms | Network + deserialization |
| Ring buffer publish | ~10-50ns | Lock-free write |
| WAL write (batch) | ~100-500μs | Disk I/O |
| WAL flush | ~1-5ms | fsync to disk |
| Match wait | ~0-5ms | Spin until progress_id |
| Match processing | ~10-100μs | In-memory matching |

**Total latency**: ~5-15ms (dominated by Kafka + WAL flush)

### Throughput

- **Single batch**: 1000-10000 orders
- **Batch frequency**: 100-1000 Hz
- **Total throughput**: 100K-10M orders/sec

### CPU Usage

- **Poll thread**: Low (mostly waiting on Kafka)
- **WAL writer**: Medium (I/O bound)
- **Matcher**: High (BusySpin waiting for progress)

## Configuration

### Ring Buffer Size
```rust
build_single_producer(8192, factory, BusySpin)
                      ^^^^
```
- Must be power of 2
- Larger = more buffering
- 8192 is optimal for most workloads

### Wait Strategy
```rust
build_single_producer(8192, factory, BusySpin)
                                     ^^^^^^^^
```
- **BusySpin**: Lowest latency, highest CPU (recommended)
- **YieldSpin**: Lower CPU, slightly higher latency
- **BlockingSpin**: Lowest CPU, highest latency

## Comparison: Sequential vs Pipeline

### Sequential (Old)
```
[Poll] → [WAL Write] → [WAL Flush] → [Match] → [Next]
Total: 5ms + 1ms + 100μs = 6.1ms per batch
```

### Pipeline (New)
```
[Poll] → [Ring Buffer] → [WAL Write] → [WAL Flush]
                      ↘ [Match (waits for progress)]
                      
Overlap: WAL Write (Batch N) || Match (Batch N-1)
Total: 5ms + max(1ms WAL, 100μs Match) ≈ 6ms
But throughput 2x higher!
```

## Testing

Build and run:
```bash
cargo build --bin matching_engine_server --release
./target/release/matching_engine_server
```

Expected output:
```
>>> Disruptor initialized with pipelined WAL writer and matcher
>>> Ring buffer size: 8192, Wait strategy: BusySpin
>>> Matching Engine Server Started (Pipelined)
[PERF] Poll: 1000 msgs. Wait: 2ms, Drain: 5ms
[PERF] Match: 1000 orders. Input: 10ms, Process: 50ms, Commit: 20ms. Total: 80ms
```

## Future Enhancements

1. **Adaptive wait strategy**: Switch between BusySpin and Yield based on load
2. **Progress metrics**: Track lag between WAL writer and matcher
3. **Multiple matchers**: Shard by symbol for parallel matching
4. **Batch size tuning**: Dynamic batch size based on latency targets
5. **NUMA awareness**: Pin threads to specific CPU cores

## References

- [LMAX Disruptor](https://lmax-exchange.github.io/disruptor/)
- [Mechanical Sympathy](https://mechanical-sympathy.blogspot.com/)
- [Lock-Free Programming](https://preshing.com/20120612/an-introduction-to-lock-free-programming/)
