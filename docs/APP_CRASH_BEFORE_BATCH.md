# App Crash Before BATCH Write - The Real Challenge

## The Critical Scenario

```rust
// Settlement Service receives trade from ZMQ
let trade = receive_trade_from_zmq()?;

// ⚡ APP CRASHES HERE - before batch is sent to ScyllaDB
let batch = create_settlement_batch(symbol, &trade);
session.batch(&batch, ()).await?;
```

**Problem**:
- Trade received from Matching Engine
- App crashes before writing to ScyllaDB
- **Trade is lost!**

This is **NOT** solved by ScyllaDB's LOGGED batch (batch never reached DB).

---

## Root Cause: At-Most-Once Delivery

### Current ZMQ Flow

```
Matching Engine → ZMQ PUB → Settlement Service (SUB)
                              ↓
                         (receives trade)
                              ↓
                         ⚡ CRASH
                              ↓
                         (trade lost)
```

**ZMQ PUB/SUB is fire-and-forget**:
- No acknowledgment
- No retry
- No durability

**If settlement service crashes**:
- ❌ Trade message is **lost**
- ❌ Never written to ScyllaDB
- ❌ User balances **never updated**

---

## Solution 1: Write-Ahead Log (WAL) in Settlement Service

### Architecture

```
┌──────────────────────────────────────────────────────────┐
│  Settlement Service                                       │
├──────────────────────────────────────────────────────────┤
│  1. Receive trade from ZMQ                               │
│  2. Write to local WAL (fsync)          ← Durable!       │
│  3. Write BATCH to ScyllaDB                              │
│  4. Mark WAL entry as committed                          │
└──────────────────────────────────────────────────────────┘
```

### Implementation

```rust
// WAL entry
#[derive(Serialize, Deserialize)]
struct WalEntry {
    sequence: u64,
    symbol: String,
    trade: MatchExecData,
    status: WalStatus,
}

enum WalStatus {
    Pending,    // Received but not written to ScyllaDB
    Committed,  // Written to ScyllaDB
}

pub struct SettlementWal {
    file: File,
    entries: HashMap<u64, WalEntry>,
}

impl SettlementWal {
    pub async fn append(&mut self, trade: &MatchExecData) -> Result<()> {
        let entry = WalEntry {
            sequence: trade.output_sequence,
            symbol: get_symbol(&trade),
            trade: trade.clone(),
            status: WalStatus::Pending,
        };

        // Write to WAL file
        let json = serde_json::to_string(&entry)?;
        self.file.write_all(json.as_bytes())?;
        self.file.write_all(b"\n")?;

        // CRITICAL: fsync to ensure durability
        self.file.sync_all()?;

        self.entries.insert(entry.sequence, entry);
        Ok(())
    }

    pub async fn mark_committed(&mut self, sequence: u64) -> Result<()> {
        if let Some(entry) = self.entries.get_mut(&sequence) {
            entry.status = WalStatus::Committed;
        }
        Ok(())
    }

    pub async fn recover(&mut self) -> Result<Vec<MatchExecData>> {
        // Read WAL file
        let content = std::fs::read_to_string(&self.file)?;

        let mut pending_trades = Vec::new();

        for line in content.lines() {
            let entry: WalEntry = serde_json::from_str(line)?;

            if entry.status == WalStatus::Pending {
                // This trade was received but not committed
                pending_trades.push(entry.trade);
            }
        }

        Ok(pending_trades)
    }
}
```

### Settlement Service with WAL

```rust
#[tokio::main]
async fn main() {
    let mut wal = SettlementWal::new("data/settlement.wal")?;

    // On startup: Recover pending trades
    let pending_trades = wal.recover().await?;
    log::info!("Recovered {} pending trades from WAL", pending_trades.len());

    for trade in pending_trades {
        // Retry settlement
        settle_trade_with_wal(&settlement_db, &mut wal, &trade).await?;
    }

    // Main loop
    loop {
        let trade = receive_trade_from_zmq()?;

        // Process with WAL
        settle_trade_with_wal(&settlement_db, &mut wal, &trade).await?;
    }
}

async fn settle_trade_with_wal(
    db: &SettlementDb,
    wal: &mut SettlementWal,
    trade: &MatchExecData,
) -> Result<()> {
    // 1. Write to WAL first (durable)
    wal.append(trade).await?;

    // 2. Write to ScyllaDB
    db.settle_trade_atomically(symbol, trade).await?;

    // 3. Mark as committed in WAL
    wal.mark_committed(trade.output_sequence).await?;

    Ok(())
}
```

**Crash Recovery**:
```
1. App crashes after WAL write but before ScyllaDB write
2. App restarts
3. WAL recovery finds pending trades
4. Pending trades are re-settled
5. ✅ No data loss
```

**Pros**:
- ✅ **No data loss** even if app crashes
- ✅ **Automatic recovery** on restart
- ✅ Simple implementation

**Cons**:
- ⚠️ Adds latency (fsync on every trade)
- ⚠️ WAL file grows (need compaction)
- ⚠️ Single point of failure (local disk)

---

## Solution 2: Kafka as Durable Queue (Recommended)

### Architecture

```
Matching Engine → Kafka Topic → Settlement Service
                     ↓
                (durable, replicated)
                     ↓
              Settlement Service crashes
                     ↓
              Settlement Service restarts
                     ↓
              Kafka replays from last committed offset
                     ↓
              ✅ No data loss
```

### Why Kafka Solves This

**Kafka provides**:
1. **Durability**: Messages written to disk
2. **Replication**: Multiple copies across brokers
3. **Offset tracking**: Consumer tracks what's been processed
4. **Replay**: Can re-consume from any offset

### Implementation

```rust
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::Message;

#[tokio::main]
async fn main() {
    let consumer: StreamConsumer = ClientConfig::new()
        .set("group.id", "settlement-service")
        .set("bootstrap.servers", "localhost:9092")
        .set("enable.auto.commit", "false")  // Manual commit for safety
        .create()?;

    consumer.subscribe(&["trades"])?;

    loop {
        let message = consumer.recv().await?;
        let trade: MatchExecData = serde_json::from_slice(message.payload())?;

        // Settle trade
        match settle_trade(&settlement_db, &trade).await {
            Ok(_) => {
                // CRITICAL: Only commit offset after successful settlement
                consumer.commit_message(&message, CommitMode::Sync)?;
                log::info!("Settled trade {}, committed offset", trade.trade_id);
            }
            Err(e) => {
                log::error!("Failed to settle trade {}: {}", trade.trade_id, e);
                // Don't commit offset - will retry on restart
            }
        }
    }
}
```

**Crash Recovery**:
```
1. App crashes after receiving trade but before committing offset
2. App restarts
3. Kafka consumer resumes from last committed offset
4. Trade is re-delivered
5. Idempotency check prevents duplicate settlement
6. ✅ No data loss
```

**Pros**:
- ✅ **No data loss** (Kafka is durable)
- ✅ **Distributed** (replicated across brokers)
- ✅ **Automatic replay** (offset management)
- ✅ **No local WAL needed**

**Cons**:
- ⚠️ Requires Kafka infrastructure
- ⚠️ More complex than ZMQ

---

## Solution 3: Matching Engine WAL + Replay (Current Architecture)

### Your Current Setup

```
Matching Engine has WAL → Can replay trades
```

**If Settlement Service crashes**:
1. Settlement Service restarts
2. Detects gap in `output_sequence`
3. **Requests replay** from Matching Engine
4. Matching Engine replays from WAL
5. ✅ No data loss

### Implementation

```rust
// Settlement Service
loop {
    let trade = receive_trade_from_zmq()?;

    // Check for sequence gap
    if trade.output_sequence != next_sequence {
        log::error!(
            "GAP DETECTED! Expected: {}, Got: {}",
            next_sequence,
            trade.output_sequence
        );

        // Request replay from Matching Engine
        request_replay(next_sequence, trade.output_sequence - 1).await?;
    }

    // Settle trade
    settle_trade(&settlement_db, &trade).await?;
    next_sequence = trade.output_sequence + 1;
}

async fn request_replay(start_seq: u64, end_seq: u64) -> Result<()> {
    // Send replay request to Matching Engine
    let request = ReplayRequest { start_seq, end_seq };
    zmq_req_socket.send(serde_json::to_vec(&request)?)?;

    // Receive replayed trades
    for seq in start_seq..=end_seq {
        let trade = receive_trade_from_zmq()?;
        settle_trade(&settlement_db, &trade).await?;
    }

    Ok(())
}
```

**Pros**:
- ✅ **No additional infrastructure** (uses existing ME WAL)
- ✅ **Source of truth** is Matching Engine
- ✅ **No data loss** (ME WAL is durable)

**Cons**:
- ⚠️ Requires **manual replay request**
- ⚠️ Matching Engine must support replay API
- ⚠️ Gap detection is reactive (not proactive)

---

## Comparison

| Solution | Data Loss Prevention | Complexity | Infrastructure | Recovery |
|----------|---------------------|------------|----------------|----------|
| **Settlement WAL** | ✅ Yes | Medium | None | Automatic |
| **Kafka** | ✅ Yes | Low | Kafka cluster | Automatic |
| **ME Replay** | ✅ Yes | High | None | Manual |
| **Current (ZMQ only)** | ❌ No | Low | None | ❌ None |

---

## Recommended Approach

### Short Term: **Settlement WAL**

```rust
// Quick win - add local WAL to settlement service
async fn settle_trade_with_wal(
    db: &SettlementDb,
    wal: &mut SettlementWal,
    trade: &MatchExecData,
) -> Result<()> {
    // 1. Durable write to WAL
    wal.append(trade).await?;

    // 2. Settle to ScyllaDB
    db.settle_trade_atomically(symbol, trade).await?;

    // 3. Mark committed
    wal.mark_committed(trade.output_sequence).await?;

    Ok(())
}
```

### Long Term: **Kafka**

```rust
// Production-grade - use Kafka for durability
async fn settle_from_kafka(
    consumer: &StreamConsumer,
    db: &SettlementDb,
) -> Result<()> {
    loop {
        let msg = consumer.recv().await?;
        let trade: MatchExecData = serde_json::from_slice(msg.payload())?;

        // Settle
        db.settle_trade_atomically(symbol, &trade).await?;

        // Commit offset (only after successful settlement)
        consumer.commit_message(&msg, CommitMode::Sync)?;
    }
}
```

---

## Summary

**Your question**: What if app crashes **before** writing batch?

**Answer**:
- ❌ **Current setup (ZMQ only)**: Trade is **lost**
- ✅ **With Settlement WAL**: Trade is **recovered** on restart
- ✅ **With Kafka**: Trade is **replayed** from last offset
- ✅ **With ME Replay**: Trade is **requested** from Matching Engine

**Recommendation**:
1. **Immediate**: Add Settlement WAL (simple, effective)
2. **Production**: Migrate to Kafka (industry standard)

**Both solutions ensure**:
- ✅ No data loss even if app crashes before DB write
- ✅ Automatic recovery on restart
- ✅ At-least-once delivery guarantee

Would you like me to implement the Settlement WAL approach?
