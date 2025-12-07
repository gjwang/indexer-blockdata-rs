# Symbol-Based SOT Sharding Implementation Plan

## Overview
Enable parallel SOT writes by sharding the `engine_output_log` table by `symbol_id`. This will allow 4-8x throughput improvement from ~10K msg/s to ~40-80K msg/s.

## Current Architecture

### Single Global Chain
```
All symbols → Single hash chain → Single SOT writer
Output 1 (BTC) → Output 2 (ETH) → Output 3 (SOL) → ...
SEQUENTIAL: ~5-10K msg/s
```

### Bottleneck
- SOT write: 20-50ms per batch (500 outputs)
- Must be sequential to maintain hash chain
- Cannot parallelize

## Proposed Architecture

### Per-Symbol Chains
```
BTC outputs → BTC chain → BTC SOT writer (shard 0)
ETH outputs → ETH chain → ETH SOT writer (shard 1)  ← PARALLEL!
SOL outputs → SOL chain → SOL SOT writer (shard 2)

Each symbol maintains its own hash chain
```

### Benefits
- **4-8x throughput**: Multiple writers in parallel
- **Hash chain preserved**: Per symbol
- **Clean partitioning**: Natural symbol boundary
- **Scalable**: Add more symbols = more parallelism

## Implementation Steps

### 1. Schema Migration

#### Update `engine_output_log` table:
```sql
-- NEW schema
CREATE TABLE IF NOT EXISTS engine_output_log (
    symbol_id int,          -- Partition key (NEW!)
    output_seq bigint,      -- Clustering key (per symbol)
    hash bigint,
    prev_hash bigint,
    input_data blob,
    output_data blob,
    created_at bigint,
    PRIMARY KEY ((symbol_id), output_seq)  -- Partitioned by symbol!
) WITH CLUSTERING ORDER BY (output_seq ASC);

-- Index for cross-symbol queries (optional)
CREATE INDEX IF NOT EXISTS idx_output_seq ON engine_output_log (output_seq);
```

#### Migration Script:
```sql
-- 1. Create new table
CREATE TABLE engine_output_log_v2 (...);

-- 2. Migrate existing data (assign symbol_id from trades)
INSERT INTO engine_output_log_v2
SELECT
    COALESCE(first_trade_symbol_id, 0) as symbol_id,  -- 0 = unknown
    output_seq,
    hash,
    prev_hash,
    input_data,
    output_data,
    created_at
FROM engine_output_log_v1;

-- 3. Swap tables
DROP TABLE engine_output_log;
ALTER TABLE engine_output_log_v2 RENAME TO engine_output_log;
```

### 2. Code Changes

#### Add symbol_id to EngineOutput
```rust
pub struct EngineOutput {
    pub output_seq: u64,
    pub prev_hash: u64,
    pub symbol_id: u32,  // NEW: Primary trading symbol for this output
    pub input: InputBundle,
    pub order_update: Option<OrderUpdate>,
    pub trades: Vec<TradeOutput>,
    pub balance_events: Vec<BalanceEvent>,
    pub hash: u64,
}
```

**How to determine symbol_id:**
- If output has trades: Use first trade's symbol_id
- If order update: Use order's symbol_id
- If balance only: Use symbol_id from input (or 0 for multi-symbol)

#### Update EngineOutputBuilder
```rust
impl EngineOutputBuilder {
    pub fn build(self) -> EngineOutput {
        // Determine primary symbol_id
        let symbol_id = self.trades.first()
            .map(|t| t.symbol_id)
            .or_else(|| self.order_update.as_ref().map(|o| o.symbol_id))
            .unwrap_or(0); // 0 = system/multi-symbol operations

        EngineOutput {
            symbol_id,
            // ... rest of fields
        }
    }
}
```

### 3. Per-Symbol SOT Writers

#### Configuration
```rust
// In settlement_service.rs
const NUM_SOT_SHARDS: usize = 8;  // Support up to 8 concurrent symbols
```

#### Spawn Writers
```rust
fn spawn_sot_writers(db: Arc<SettlementDb>) -> Vec<mpsc::Sender<EngineOutput>> {
    let mut sot_channels = Vec::with_capacity(NUM_SOT_SHARDS);

    for shard_id in 0..NUM_SOT_SHARDS {
        let (tx, mut rx) = mpsc::channel::<EngineOutput>(1000);
        sot_channels.push(tx);

        let db = db.clone();
        tokio::spawn(async move {
            // Per-symbol hash chain state
            let mut last_hash = 0u64;
            let mut last_seq = 0u64;

            while let Some(output) = rx.recv().await {
                // Verify hash chain (per symbol)
                if output.prev_hash != last_hash {
                    log::error!("Hash chain break for symbol_id={}", output.symbol_id);
                    continue;
                }

                // Write to ScyllaDB
                if let Err(e) = db.write_engine_output(&output).await {
                    log::error!("SOT write failed for shard {}: {}", shard_id, e);
                }

                last_hash = output.hash;
                last_seq = output.output_seq;
            }
        });
    }

    sot_channels
}
```

#### Route by symbol_id
```rust
async fn write_sot_batch(outputs: &[EngineOutput], sot_writers: &[mpsc::Sender<EngineOutput>]) {
    // Group by symbol_id
    use std::collections::HashMap;
    let mut by_symbol: HashMap<u32, Vec<&EngineOutput>> = HashMap::new();

    for output in outputs {
        by_symbol.entry(output.symbol_id)
            .or_insert_with(Vec::new)
            .push(output);
    }

    // Send each symbol group to its shard
    for (symbol_id, symbol_outputs) in by_symbol {
        let shard = (symbol_id as usize) % sot_writers.len();

        for output in symbol_outputs {
            let _ = sot_writers[shard].send(output.clone()).await;
        }
    }
}
```

### 4. Database Layer Updates

#### Update write_engine_output
```rust
// In settlement_db.rs
pub async fn write_engine_output(&self, output: &EngineOutput) -> Result<()> {
    let now = get_current_timestamp_ms();

    // Serialize and compress
    let binary = bincode::serialize(output)?;
    let compressed = lz4_flex::compress_prepend_size(&binary);

    self.session
        .query(
            "INSERT INTO engine_output_log (symbol_id, output_seq, hash, prev_hash, output_data, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            (
                output.symbol_id as i32,  // NEW!
                output.output_seq as i64,
                output.hash as i64,
                output.prev_hash as i64,
                compressed,
                now as i64,
            ),
        )
        .await?;

    Ok(())
}
```

#### Update batch write
```rust
pub async fn write_engine_outputs_batch(&self, outputs: &[EngineOutput]) -> Result<()> {
    // Group by symbol_id for optimal batching
    use std::collections::HashMap;
    let mut by_symbol: HashMap<u32, Vec<&EngineOutput>> = HashMap::new();

    for output in outputs {
        by_symbol.entry(output.symbol_id).or_default().push(output);
    }

    // Write each symbol's batch
    for (symbol_id, symbol_outputs) in by_symbol {
        let mut batch = Batch::new(BatchType::Unlogged);
        let stmt = "INSERT INTO engine_output_log (symbol_id, output_seq, hash, prev_hash, output_data, created_at) VALUES (?, ?, ?, ?, ?, ?)";

        for _ in &symbol_outputs {
            batch.append_statement(stmt);
        }

        let values: Vec<_> = symbol_outputs.iter().map(|o| {
            let binary = bincode::serialize(o).unwrap();
            let compressed = lz4_flex::compress_prepend_size(&binary);
            (
                symbol_id as i32,
                o.output_seq as i64,
                o.hash as i64,
                o.prev_hash as i64,
                compressed,
                get_current_timestamp_ms() as i64,
            )
        }).collect();

        self.session.batch(&batch, values).await?;
    }

    Ok(())
}
```

### 5. Recovery Logic Updates

#### Per-Symbol Recovery
```rust
pub async fn recover_from_sot(&self) -> Result<Vec<EngineOutput>> {
    let mut all_outputs = Vec::new();

    // Recover each symbol independently
    for symbol_id in 0..NUM_SOT_SHARDS {
        let symbol_outputs = self.recover_symbol_chain(symbol_id as u32).await?;
        all_outputs.extend(symbol_outputs);
    }

    // Sort by global output_seq for final replay
    all_outputs.sort_by_key(|o| o.output_seq);

    Ok(all_outputs)
}

async fn recover_symbol_chain(&self, symbol_id: u32) -> Result<Vec<EngineOutput>> {
    // Read all outputs for this symbol
    let rows = self.session
        .query(
            "SELECT output_data FROM engine_output_log WHERE symbol_id = ? ORDER BY output_seq ASC",
            (symbol_id as i32,)
        )
        .await?;

    let mut outputs = Vec::new();
    let mut expected_prev_hash = 0u64;

    for row in rows.rows()? {
        let compressed: Vec<u8> = row.columns[0].as_ref().unwrap().as_blob().unwrap().to_vec();
        let binary = lz4_flex::decompress_size_prepended(&compressed)?;
        let output: EngineOutput = bincode::deserialize(&binary)?;

        // Verify hash chain (per symbol)
        if output.prev_hash != expected_prev_hash {
            return Err(anyhow!("Hash chain broken for symbol {} at seq {}", symbol_id, output.output_seq));
        }

        expected_prev_hash = output.hash;
        outputs.push(output);
    }

    log::info!("Recovered {} outputs for symbol_id {}", outputs.len(), symbol_id);
    Ok(outputs)
}
```

## Performance Impact

### Current Performance
- SOT write: 1 writer, 20-50ms per batch
- Throughput: ~10K msg/s (bottlenecked by balance writes now)

### Expected Performance
- SOT write: 8 parallel writers, 20-50ms per batch
- Throughput: **40-80K msg/s** (8x parallelism)
- Balance writes: Still ~10K msg/s (becomes new bottleneck)

### Next Bottleneck
After SOT sharding, balance writes become the bottleneck again. Solution:
- Increase `NUM_BALANCE_WRITERS` from 4 to 16
- Scale ScyllaDB cluster (3+ nodes)
- Use consistency level `LOCAL_ONE`

## Migration Path

### Phase 1: Schema Update (Zero Downtime)
1. Deploy new schema alongside old
2. Dual-write to both tables
3. Backfill old data
4. Switch reads to new table
5. Drop old table

### Phase 2: Code Deployment
1. Deploy new code with symbol_id support
2. Monitor per-symbol chains
3. Verify recovery works

### Phase 3: Scale Testing
1. Test with 8 concurrent symbols
2. Verify 40K+ msg/s throughput
3. Monitor ScyllaDB cluster health

## Risks & Mitigation

### Risk 1: Data Migration
**Risk**: Existing data doesn't have symbol_id
**Mitigation**:
- Extract from trades in EngineOutput
- Use symbol_id=0 for unknown/multi-symbol ops
- Test migration on staging first

### Risk 2: Hash Chain Breaks
**Risk**: Bug in per-symbol chain logic
**Mitigation**:
- Comprehensive unit tests
- Recovery verification
- Canary deployment

### Risk 3: Uneven Symbol Distribution
**Risk**: One symbol gets 90% of traffic
**Mitigation**:
- Monitor per-shard throughput
- Dynamic shard assignment (future)
- Use modulo distribution for now

## Testing Plan

### Unit Tests
- Per-symbol hash chain verification
- symbol_id extraction logic
- Recovery from multiple chains

### Integration Tests
- Multi-symbol concurrent writes
- Full recovery test
- Performance benchmark (target: 40K msg/s)

### Load Tests
- 8 symbols, equal distribution
- 1 hot symbol scenario
- Recovery during load

## Timeline

- **Day 1**: Schema design + migration script
- **Day 2**: EngineOutput changes + SOT writer logic
- **Day 3**: Database layer updates
- **Day 4**: Recovery logic + testing
- **Day 5**: Integration testing + deployment

**Total: 5 days to implement and test**

## Metrics to Track

### Before
- SOT throughput: ~10K msg/s
- SOT write latency: 20-50ms
- Writers: 1 sequential

### After
- SOT throughput: **40-80K msg/s**
- SOT write latency: 20-50ms (unchanged per writer)
- Writers: 8 parallel
- Improvement: **4-8x faster**

## Future Enhancements

### Dynamic Symbol Routing
- Monitor symbol traffic patterns
- Assign hot symbols to dedicated shards
- Load balancing across writers

### Cross-Symbol Dependencies
- Handle multi-symbol atomic operations
- Distributed transactions if needed
- 2-phase commit for complex scenarios

### Horizontal Scaling
- Multiple settlement services
- Partition symbols across instances
- Kafka-based distributed sequencing

---

## Appendix: Example Scenarios

### Scenario 1: BTC/USDT Trade
```
Input: Place BTC/USDT order
Output:
  - symbol_id = 1 (BTC/USDT)
  - Routed to: shard 1 % 8 = 1
  - Written to: engine_output_log WHERE symbol_id=1
```

### Scenario 2: Multi-Symbol Balance Update
```
Input: Deposit USDT (affects all pairs)
Output:
  - symbol_id = 0 (system operation)
  - Routed to: shard 0
  - Written to: engine_output_log WHERE symbol_id=0
```

### Scenario 3: Recovery
```
1. Read from symbol_id=0 (system)
2. Read from symbol_id=1 (BTC/USDT)
3. Read from symbol_id=2 (ETH/USDT)
...
8. Merge all chains by output_seq
9. Replay in sequence order
```
