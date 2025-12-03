# Disruptor Pattern Refactoring

## Overview

Refactored the matching engine server to use a **disruptor pattern** with concurrent poll and process threads. The key innovation is using a **progress ID** as a gate to ensure all orders are persisted in the WAL before matching.

## Architecture

### Two Concurrent Threads

1. **Poll Thread** (Main thread)
   - Polls messages from Kafka
   - Assigns sequence numbers to each order
   - Writes orders to WAL (no flush yet)
   - Batches orders and flushes WAL
   - Updates progress ID after successful flush
   - Sends `(sequence_id, command)` to process thread

2. **Process Thread** (Spawned task)
   - Receives `(sequence_id, command)` messages
   - Stores commands in a BTreeMap ordered by sequence
   - Reads progress ID to check what's been persisted
   - Processes only commands with `seq <= progress_id`
   - Ensures orders are only matched after WAL persistence

### Progress ID as Gate Keeper

The **progress ID** is an `Arc<AtomicU64>` that acts as a synchronization point:

- **Poll thread**: Updates progress ID after flushing WAL
- **Process thread**: Only processes orders up to the current progress ID
- **Guarantee**: Every order is persisted before matching

## Key Changes

### 1. WAL Progress Tracking (`order_wal.rs`)

```rust
pub struct Wal {
    writer: BufWriter<File>,
    pub current_seq: u64,
    progress_id: Arc<AtomicU64>,  // NEW: Track flushed position
}

impl Wal {
    pub fn flush(&mut self) -> Result<(), std::io::Error> {
        self.writer.flush()?;
        // Update progress after successful flush
        self.progress_id.store(self.current_seq, Ordering::Release);
        Ok(())
    }

    pub fn get_progress_handle(&self) -> Arc<AtomicU64> {
        self.progress_id.clone()
    }
}
```

### 2. Sequence Assignment (Poll Thread)

```rust
// Assign sequence number before WAL write
current_seq += 1;
batch_seq = current_seq;

// Write to WAL (no flush)
order_wal.log_place_order_no_flush(...);

// Later: Flush and send
order_wal.flush()?;  // This updates progress_id
tx.send((batch_seq, EngineCommand::PlaceOrderBatch(orders))).await?;
```

### 3. Progress-Gated Processing (Process Thread)

```rust
let mut pending_commands: BTreeMap<u64, EngineCommand> = BTreeMap::new();

loop {
    // Receive new commands
    while let Ok((seq, cmd)) = rx.try_recv() {
        pending_commands.insert(seq, cmd);
    }
    
    // Check what's been flushed
    let progress = progress_handle.load(Ordering::Acquire);
    
    // Process only flushed commands
    while let Some((&seq, _)) = pending_commands.first_key_value() {
        if seq > progress {
            break;  // Not flushed yet, wait
        }
        
        if let Some((_, cmd)) = pending_commands.pop_first() {
            // Process command (matching, etc.)
            engine.add_order_batch(batch);
        }
    }
}
```

## Benefits

1. **Durability**: All orders are guaranteed to be in WAL before matching
2. **Concurrency**: Poll and process threads run in parallel
3. **Performance**: Batched WAL writes reduce I/O overhead
4. **Ordering**: Sequence numbers ensure correct processing order
5. **Recovery**: WAL can be replayed to restore state

## Flow Diagram

```
Kafka → Poll Thread → WAL Write → Flush → Update Progress ID
                         ↓                        ↓
                    Assign Seq              (seq, cmd)
                                                  ↓
                                         Process Thread
                                                  ↓
                                    Wait for seq <= progress_id
                                                  ↓
                                         Match Orders
```

## Testing

Build and run:

```bash
cargo build --bin matching_engine_server --release
./target/release/matching_engine_server
```

Expected output:
```
>>> Process Thread Started
>>> Matching Engine Server Started (Pipelined)
[PERF] Poll: 1000 msgs. Wait: 2ms, Drain: 5ms
[PERF] Match: 1000 orders. Input: 10ms, Process: 50ms, Commit: 20ms. Total: 80ms
```

## Future Enhancements

1. **Metrics**: Track progress lag (current_seq - progress_id)
2. **Backpressure**: Slow down poll if process thread falls behind
3. **Multiple WALs**: Shard by symbol for higher throughput
4. **Lock-free queue**: Replace MPSC with LMAX disruptor for even better performance
