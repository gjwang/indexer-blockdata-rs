# Debug Logging Added to Pipeline Architecture

## Changes Made

Added comprehensive debug logging to track the flow of events through the pipeline:

### 1. WAL Writer Logging

```rust
[WAL Writer] Processing batch of N orders at seq=X
[WAL Writer] Processing cancel order ID at seq=X
[WAL Writer] Processing balance request at seq=X
[WAL Writer] WARNING: Empty event at seq=X
[WAL Writer] Flushing WAL at seq=X, current_seq=Y
[WAL Writer] Progress updated to Z
```

### 2. Matcher Logging

```rust
[Matcher] Waited N iterations for seq=X, progress=Y
[Matcher] Still waiting for seq=X, progress=Y  (every 1M iterations)
[Matcher] Matching batch of N orders at seq=X
[Matcher] Canceling order ID at seq=X
[Matcher] Processing balance request at seq=X
[Matcher] WARNING: Empty event at seq=X
```

### 3. Poll Thread Logging

```rust
[Poll] Publishing batch of N orders before cancel
[Poll] Publishing cancel order ID
[Poll] Publishing final batch of N orders
```

## How to Test

### 1. Start the Matching Engine Server

```bash
cd /Users/gjwang/eclipse-workspace/rust_source/indexer-blockdata-rs
./target/release/matching_engine_server
```

Expected startup output:
```
>>> Disruptor initialized with pipelined WAL writer and matcher
>>> Ring buffer size: 8192, Wait strategy: BusySpin
>>> Matching Engine Server Started (Pipelined)
```

### 2. Send Test Orders

Use the order HTTP client to send orders:

```bash
# In another terminal
./target/release/order_http_client
```

### 3. Expected Log Flow

You should see logs in this order:

```
[PERF] Poll: 1000 msgs. Wait: 2ms, Drain: 5ms
[Poll] Publishing final batch of 1000 orders
[WAL Writer] Processing batch of 1000 orders at seq=0
[WAL Writer] Flushing WAL at seq=0, current_seq=0
[WAL Writer] Progress updated to 0
[Matcher] Matching batch of 1000 orders at seq=0
[PERF] Match: 1000 orders. Input: 10ms, Process: 50ms, Commit: 20ms. Total: 80ms
```

## What to Look For

### ✅ Good Signs

1. **Poll logs appear** - Messages are being received from Kafka
2. **WAL Writer logs appear** - Events are being consumed and written to WAL
3. **Progress updates** - `Progress updated to X` shows WAL is flushing
4. **Matcher logs appear** - Orders are being matched after WAL flush
5. **No "WARNING: Empty event"** - All events have commands

### ❌ Problem Signs

1. **Only Poll logs, no WAL Writer** - Events not reaching disruptor consumers
2. **WAL Writer but no Matcher** - Matcher stuck waiting for progress
3. **"WARNING: Empty event"** - Events being published without commands
4. **Matcher "Still waiting"** - Progress ID not being updated

## Debugging Steps

### If No WAL Writer Logs

The disruptor consumers aren't running. Check:
- Is the disruptor builder correct?
- Are the closures being moved properly?

### If No Matcher Logs

The matcher is stuck waiting. Check:
- Is `progress_id` being updated in `order_wal.flush()`?
- Is the matcher checking the right progress value?
- Add more logging to see progress values

### If Empty Events

Events are being published without commands. Check:
- Is `producer.publish(|event| { event.command = Some(...) })` being called?
- Are the closures capturing the right variables?

## Performance Impact

The debug logging will reduce performance due to:
- Console I/O overhead
- String formatting
- Synchronization for stdout

**For production**, remove or disable these logs and use:
- Metrics/counters instead of println
- Sampling (log every Nth event)
- Async logging

## Next Steps

Once verified working:
1. Remove or reduce debug logging
2. Add performance metrics
3. Test with high load
4. Monitor progress lag (current_seq - progress_id)
