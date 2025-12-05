# BATCH Performance Analysis

## Performance Cost of LOGGED Batch

### LOGGED Batch Overhead

```rust
// LOGGED batch (slower but safe)
let mut batch = BatchStatement::new(BatchType::Logged);
```

**What happens**:
1. **Write to batch log** (extra disk I/O)
2. Execute all statements
3. Mark batch as complete
4. Remove from batch log

**Performance impact**:
- ⚠️ **~2-3x slower** than individual writes
- ⚠️ **Extra disk I/O** for batch log
- ⚠️ **Serialization overhead**

### Benchmark Example

```
Individual writes:  1000 trades/sec
LOGGED batch:       300-500 trades/sec  (2-3x slower)
UNLOGGED batch:     800-1000 trades/sec (similar to individual)
```

---

## Is This Acceptable?

### For HFT Exchange?

**It depends on your throughput requirements**:

| Scenario | Trades/Sec | LOGGED Batch OK? |
|----------|------------|------------------|
| Low volume | < 100/sec | ✅ Yes, plenty of headroom |
| Medium volume | 100-500/sec | ✅ Yes, acceptable |
| High volume | 500-1000/sec | ⚠️ Maybe, need optimization |
| Ultra-high | > 1000/sec | ❌ No, need different approach |

**Your current architecture**:
- Matching Engine: ~10,000+ orders/sec (in-memory)
- Settlement Service: Only settled **trades** (much lower volume)
- Typical: 10-100 trades/sec (depends on matching rate)

**Verdict**: ✅ **LOGGED batch is likely fine** for settlement

---

## Optimization Strategies

### Option 1: Micro-Batching (Best for Performance)

Instead of settling each trade individually, **batch multiple trades together**:

```rust
async fn settlement_loop(
    db: &SettlementDb,
    subscriber: &zmq::Socket,
) -> Result<()> {
    let mut trade_buffer = Vec::new();
    let mut last_flush = Instant::now();

    loop {
        // Receive trade (non-blocking)
        match subscriber.recv_bytes(zmq::DONTWAIT) {
            Ok(data) => {
                let trade: MatchExecData = serde_json::from_slice(&data)?;
                trade_buffer.push(trade);
            }
            Err(zmq::Error::EAGAIN) => {
                // No more messages, check if we should flush
            }
            Err(e) => return Err(e.into()),
        }

        // Flush conditions
        let should_flush =
            trade_buffer.len() >= 10 ||  // Buffer full
            last_flush.elapsed() > Duration::from_millis(100);  // Timeout

        if should_flush && !trade_buffer.is_empty() {
            // Settle all trades in one big batch
            settle_trades_batch(db, &trade_buffer).await?;
            trade_buffer.clear();
            last_flush = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn settle_trades_batch(
    db: &SettlementDb,
    trades: &[MatchExecData],
) -> Result<()> {
    // Create one big LOGGED batch for all trades
    let mut batch = BatchStatement::new(BatchType::Logged);

    for trade in trades {
        // Add 5 statements per trade (1 insert + 4 balance updates)
        batch.append(insert_trade_stmt(trade));
        batch.append(update_balance_stmt(/* buyer BTC */));
        batch.append(update_balance_stmt(/* buyer USDT */));
        batch.append(update_balance_stmt(/* seller BTC */));
        batch.append(update_balance_stmt(/* seller USDT */));
    }

    // Execute one batch for all trades
    db.session.batch(&batch, ()).await?;

    Ok(())
}
```

**Performance**:
- 10 trades = 50 statements in one batch
- **Amortizes batch log overhead** across multiple trades
- **Throughput**: 1000+ trades/sec

**Trade-off**:
- ⚠️ Slightly higher latency (up to 100ms)
- ✅ Much higher throughput
- ✅ Still atomic (all trades in batch succeed or fail together)

---

### Option 2: UNLOGGED Batch + Application-Level Idempotency

Use **UNLOGGED batch** (faster) but add **application-level recovery**:

```rust
async fn settle_trade_unlogged(
    db: &SettlementDb,
    wal: &mut SettlementWal,
    trade: &MatchExecData,
) -> Result<()> {
    // 1. Write to application WAL first (durable)
    wal.append(trade).await?;

    // 2. Use UNLOGGED batch (faster, no DB-level atomicity)
    let mut batch = BatchStatement::new(BatchType::Unlogged);
    batch.append(insert_trade_stmt(trade));
    batch.append(update_balance_stmt(/* ... */));
    // ... (4 balance updates)

    // 3. Execute batch
    db.session.batch(&batch, ()).await?;

    // 4. Mark as committed in WAL
    wal.mark_committed(trade.output_sequence).await?;

    Ok(())
}

// On startup: Recover from WAL
async fn recover_from_wal(db: &SettlementDb, wal: &SettlementWal) -> Result<()> {
    let pending = wal.get_pending_trades().await?;

    for trade in pending {
        // Check if trade exists in DB
        if db.trade_exists(&trade.trade_id).await? {
            // Trade inserted, check balances
            verify_and_fix_balances(db, &trade).await?;
        } else {
            // Trade not inserted, settle it
            settle_trade_unlogged(db, wal, &trade).await?;
        }
    }

    Ok(())
}
```

**Performance**:
- ✅ **~3x faster** than LOGGED batch
- ✅ **Similar to individual writes**

**Trade-off**:
- ⚠️ Need application WAL
- ⚠️ Manual recovery logic
- ⚠️ More complex

---

### Option 3: Hybrid - Per-User Batching

Batch updates **per user** instead of per trade:

```rust
// Accumulate balance changes per user
struct UserBalanceChanges {
    user_id: u64,
    btc_delta: i64,
    usdt_delta: i64,
}

async fn settle_trades_with_user_batching(
    db: &SettlementDb,
    trades: &[MatchExecData],
) -> Result<()> {
    // 1. Insert all trades (can be UNLOGGED, trades are idempotent by trade_id)
    let mut trade_batch = BatchStatement::new(BatchType::Unlogged);
    for trade in trades {
        trade_batch.append(insert_trade_stmt(trade));
    }
    db.session.batch(&trade_batch, ()).await?;

    // 2. Aggregate balance changes per user
    let mut user_changes: HashMap<u64, UserBalanceChanges> = HashMap::new();

    for trade in trades {
        // Buyer
        let buyer = user_changes.entry(trade.buyer_user_id).or_default();
        buyer.btc_delta += trade.quantity as i64;
        buyer.usdt_delta -= (trade.price * trade.quantity) as i64;

        // Seller
        let seller = user_changes.entry(trade.seller_user_id).or_default();
        seller.btc_delta -= trade.quantity as i64;
        seller.usdt_delta += (trade.price * trade.quantity) as i64;
    }

    // 3. Update balances (one update per user per asset)
    let mut balance_batch = BatchStatement::new(BatchType::Logged);
    for (user_id, changes) in user_changes {
        if changes.btc_delta != 0 {
            balance_batch.append(update_balance_stmt(user_id, BTC, changes.btc_delta));
        }
        if changes.usdt_delta != 0 {
            balance_batch.append(update_balance_stmt(user_id, USDT, changes.usdt_delta));
        }
    }
    db.session.batch(&balance_batch, ()).await?;

    Ok(())
}
```

**Performance**:
- ✅ **Fewer balance updates** (aggregated per user)
- ✅ **Smaller LOGGED batch** (only balances, not trades)
- ✅ **Higher throughput**

**Trade-off**:
- ⚠️ Trades and balances in **separate batches** (not fully atomic)
- ⚠️ Need reconciliation if balance batch fails

---

## Recommended Approach

### For Your Use Case (Settlement Service)

**Start with: Micro-Batching + LOGGED Batch**

```rust
// Settle 10 trades at a time in one LOGGED batch
async fn settlement_loop() {
    let mut buffer = Vec::new();

    loop {
        // Collect trades
        while buffer.len() < 10 {
            if let Ok(trade) = receive_trade_with_timeout(100ms) {
                buffer.push(trade);
            } else {
                break;  // Timeout, flush what we have
            }
        }

        if !buffer.is_empty() {
            // Settle all trades in one LOGGED batch
            settle_trades_batch(&buffer).await?;
            buffer.clear();
        }
    }
}
```

**Why**:
- ✅ **Atomic** (LOGGED batch)
- ✅ **Good performance** (amortized overhead)
- ✅ **Simple** (no WAL needed)
- ✅ **Acceptable latency** (< 100ms)

**If you need more performance**:
- Use **UNLOGGED batch + application WAL**
- Or use **Kafka** (provides durability + high throughput)

---

## Performance Comparison

| Approach | Throughput | Latency | Atomicity | Complexity |
|----------|------------|---------|-----------|------------|
| **Individual LOGGED** | 300/sec | 3ms | ✅ Full | Low |
| **Micro-batch LOGGED** | 1000+/sec | 100ms | ✅ Full | Low |
| **UNLOGGED + WAL** | 800/sec | 3ms | ⚠️ App-level | Medium |
| **User batching** | 1500+/sec | 50ms | ⚠️ Partial | Medium |

---

## Conclusion

**Is LOGGED batch performance bad?**
- For **individual trades**: Yes, 2-3x slower
- For **micro-batched trades**: No, acceptable (1000+ trades/sec)

**Recommendation**:
1. **Start with**: Micro-batching (10-50 trades per batch)
2. **Monitor**: Measure actual throughput
3. **Optimize if needed**: Switch to UNLOGGED + WAL

**For most exchanges**, micro-batched LOGGED batches are **sufficient** and provide **strong atomicity guarantees**.

Would you like me to implement the micro-batching approach?
