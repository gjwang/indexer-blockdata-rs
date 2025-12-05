# BATCH Failure Scenarios and Recovery

## The Critical Question

**What if trade insert succeeds but balance updates fail?**

This is the **exact problem** we're trying to solve!

## Understanding ScyllaDB BATCH Guarantees

### BATCH Types

ScyllaDB has two types of batches:

```rust
// 1. LOGGED batch (default)
BatchStatement::new(BatchType::Logged);

// 2. UNLOGGED batch
BatchStatement::new(BatchType::Unlogged);
```

### LOGGED Batch Guarantees

**LOGGED batches provide atomicity**:

```
┌─────────────────────────────────────────────────────────┐
│  LOGGED Batch Execution Flow                            │
├─────────────────────────────────────────────────────────┤
│  1. Write to batch log (durable storage)                │
│  2. Execute all statements                              │
│  3. Mark batch as complete                              │
│  4. Remove from batch log                               │
└─────────────────────────────────────────────────────────┘
```

**If crash happens**:
- ✅ **Before step 1**: Nothing written, batch never started
- ✅ **During step 2**: Batch log replays on restart → all statements re-executed
- ✅ **After step 3**: Batch completed, all data written

**Result**: **All statements succeed or all fail** (even across crashes)

---

## Scenario Analysis

### Scenario 1: Crash During BATCH Execution

```rust
let mut batch = BatchStatement::new(BatchType::Logged);
batch.append("INSERT INTO settled_trades_btc_usdt ...");
batch.append("UPDATE user_balances ...");  // 4 balance updates

session.batch(&batch, ()).await?;  // ← CRASH HERE
```

**What happens**:

1. **Batch log written** to disk (durable)
2. **Crash occurs** mid-execution
3. **ScyllaDB restarts**
4. **Batch log replayed** → all statements re-executed
5. **Trade insert** may fail (duplicate key) → OK, idempotent
6. **Balance updates** re-applied → **Consistency restored**

**Result**: ✅ **Eventually consistent** (batch completes after restart)

---

### Scenario 2: Network Partition During BATCH

```rust
session.batch(&batch, ()).await?;  // ← Network timeout
```

**What happens**:

1. Client sends batch to ScyllaDB
2. **Network fails** before response
3. Client doesn't know if batch succeeded

**Problem**: **Uncertainty** - did it succeed or not?

**Solution**: **Idempotent retry**

```rust
pub async fn settle_trade_with_retry(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    // 1. Check if trade already exists
    if self.trade_exists(symbol, trade.trade_id).await? {
        log::info!("Trade {} already settled", trade.trade_id);
        return Ok(());  // Idempotent
    }

    // 2. Execute batch
    let batch = self.create_settlement_batch(symbol, trade);

    match self.session.batch(&batch, ()).await {
        Ok(_) => Ok(()),
        Err(e) if is_duplicate_key_error(&e) => {
            // Trade was inserted, batch succeeded
            log::info!("Trade {} already settled (duplicate key)", trade.trade_id);
            Ok(())
        }
        Err(e) => {
            // Genuine failure, retry
            log::error!("Batch failed: {}, retrying...", e);
            Err(e.into())
        }
    }
}
```

---

### Scenario 3: Partial BATCH Failure (Rare)

**Can individual statements in a LOGGED batch fail?**

**Yes, but**:
- If **any** statement fails, **entire batch fails**
- ScyllaDB **rolls back** all changes
- Example: Balance goes negative → entire batch fails

```rust
// Example: Insufficient balance
batch.append("UPDATE user_balances SET available = available - 1000 WHERE user_id = 1");
// If available < 1000, this fails → ENTIRE batch fails
```

**Result**: ✅ **Atomicity preserved** (all-or-nothing)

---

## The REAL Problem: UNLOGGED Batches

### UNLOGGED Batch (DON'T USE for settlement!)

```rust
// ❌ DANGEROUS for settlement
let mut batch = BatchStatement::new(BatchType::Unlogged);
```

**No atomicity guarantee**:
- ❌ Trade insert may succeed
- ❌ Balance updates may fail
- ❌ **Inconsistent state**

**When to use UNLOGGED**:
- ✅ Inserting multiple independent rows
- ✅ Performance-critical, idempotent operations
- ❌ **NEVER for settlement** (we need atomicity)

---

## Recommended Implementation

### 1. Use LOGGED Batch

```rust
pub async fn settle_trade_atomically(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    // CRITICAL: Use LOGGED batch for atomicity
    let mut batch = BatchStatement::new(BatchType::Logged);

    // Add all statements
    batch.append(self.insert_trade_stmt(symbol, trade));
    batch.append(self.update_balance_stmt(/* buyer BTC */));
    batch.append(self.update_balance_stmt(/* buyer USDT */));
    batch.append(self.update_balance_stmt(/* seller BTC */));
    batch.append(self.update_balance_stmt(/* seller USDT */));

    // Execute atomically
    self.session.batch(&batch, ()).await?;

    Ok(())
}
```

### 2. Add Idempotency Check

```rust
pub async fn settle_trade_idempotent(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    // Pre-check: Already settled?
    if self.trade_exists(symbol, trade.trade_id).await? {
        log::debug!("Trade {} already settled, skipping", trade.trade_id);
        return Ok(());
    }

    // Execute batch
    match self.settle_trade_atomically(symbol, trade).await {
        Ok(_) => Ok(()),
        Err(e) if is_duplicate_key_error(&e) => {
            // Concurrent settlement, already done
            log::debug!("Trade {} settled concurrently", trade.trade_id);
            Ok(())
        }
        Err(e) => Err(e),
    }
}
```

### 3. Add Retry Logic

```rust
pub async fn settle_trade_with_retry(
    &self,
    symbol: &str,
    trade: &MatchExecData,
) -> Result<()> {
    const MAX_RETRIES: u32 = 3;
    let mut attempt = 0;

    loop {
        attempt += 1;

        match self.settle_trade_idempotent(symbol, trade).await {
            Ok(_) => return Ok(()),
            Err(e) if attempt >= MAX_RETRIES => {
                log::error!("Failed to settle trade {} after {} attempts: {}",
                           trade.trade_id, MAX_RETRIES, e);
                return Err(e);
            }
            Err(e) => {
                log::warn!("Attempt {}/{} failed: {}, retrying...",
                          attempt, MAX_RETRIES, e);
                tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
            }
        }
    }
}
```

---

## Recovery Mechanisms

### 1. Automatic Recovery (ScyllaDB)

**LOGGED batch log** ensures:
- Batches complete even after crash
- No manual intervention needed
- **Automatic consistency**

### 2. Manual Recovery (Application)

If somehow inconsistency occurs (e.g., bug, data corruption):

```rust
pub async fn verify_and_fix_balance(
    &self,
    user_id: u64,
    asset_id: u32,
) -> Result<()> {
    // 1. Get current balance from user_balances table
    let current_balance = self.get_user_balance(user_id, asset_id).await?;

    // 2. Calculate expected balance from trade history
    let expected_balance = self.calculate_balance_from_trades(user_id, asset_id).await?;

    // 3. Compare
    if current_balance != expected_balance {
        log::error!(
            "Balance mismatch for user {} asset {}: current={}, expected={}",
            user_id, asset_id, current_balance, expected_balance
        );

        // 4. Fix it
        self.force_update_balance(user_id, asset_id, expected_balance).await?;

        log::info!("Balance corrected for user {} asset {}", user_id, asset_id);
    }

    Ok(())
}

async fn calculate_balance_from_trades(
    &self,
    user_id: u64,
    asset_id: u32,
) -> Result<i64> {
    let mut balance = 0i64;

    // Query all symbol tables
    for symbol in ["btc_usdt", "eth_usdt", "btc_eth"] {
        let table = format!("settled_trades_{}", symbol);
        let trades = self.get_trades_for_user(&table, user_id).await?;

        for trade in trades {
            // Calculate delta based on role and asset
            balance += self.calculate_trade_delta(&trade, user_id, asset_id);
        }
    }

    Ok(balance)
}
```

---

## Monitoring and Alerts

### 1. Detect Inconsistencies

```rust
// Periodic reconciliation job
#[tokio::main]
async fn reconciliation_job() {
    loop {
        // Sleep for 1 hour
        tokio::time::sleep(Duration::from_secs(3600)).await;

        // Check random sample of users
        let sample_users = get_random_users(100).await;

        for user_id in sample_users {
            for asset_id in [BTC, USDT, ETH] {
                if let Err(e) = verify_balance(user_id, asset_id).await {
                    log::error!("Reconciliation failed: {}", e);
                    send_alert("Balance inconsistency detected", user_id, asset_id);
                }
            }
        }
    }
}
```

### 2. Metrics

```rust
// Track batch success/failure rates
metrics::counter!("settlement_batch_success").increment(1);
metrics::counter!("settlement_batch_failure").increment(1);
metrics::counter!("settlement_batch_retry").increment(1);
```

---

## Summary

| Scenario | LOGGED Batch Behavior | Recovery |
|----------|----------------------|----------|
| **Crash during batch** | ✅ Replayed from batch log | Automatic |
| **Network timeout** | ⚠️ Unknown state | Idempotent retry |
| **Duplicate insert** | ✅ Fails gracefully | Idempotent check |
| **Balance negative** | ✅ Entire batch fails | Retry or alert |
| **Data corruption** | ❌ Rare, needs detection | Manual rebuild |

## Best Practices

1. ✅ **Always use LOGGED batch** for settlement
2. ✅ **Add idempotency check** before batch
3. ✅ **Implement retry logic** with exponential backoff
4. ✅ **Monitor batch failures** with metrics
5. ✅ **Run periodic reconciliation** to detect issues
6. ✅ **Keep audit trail** in per-symbol trade tables

**With LOGGED batches, the answer to your question is**:

> **If trade insert succeeds but balance update fails, ScyllaDB's batch log ensures the balance update will be retried and completed, maintaining consistency.**

The **only** way to have inconsistency is:
- Using UNLOGGED batch (don't do this)
- Data corruption (extremely rare)
- Application bug (prevented by reconciliation)

**Recommendation**: Use LOGGED batch + idempotency + reconciliation = **Production-ready atomicity**
