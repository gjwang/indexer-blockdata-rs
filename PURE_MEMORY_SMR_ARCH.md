# High-Frequency Trading Exchange Architecture (Final Plan)

## 1. Core Philosophy: Pure Memory SMR
The architecture is based on **State Machine Replication (SMR)** with a **Pure In-Memory Matching Engine**.

*   **Input is Truth**: The Redpanda (Kafka) Input Log is the *only* persistent source of truth.
*   **Memory is Speed**: The Matching Engine (ME) holds all state in RAM. We remove the local disk WAL to eliminate I/O latency spikes.
*   **Determinism is Safety**: Multiple ME replicas process the same input stream to produce identical outputs.

## 2. System Components & Data Flow

### A. Gateway Layer (Scalable Ingest)
*   **Role**: Handles User Connections (WebSocket/REST), Auth, and Rate Limiting.
*   **Scaling**: Horizontal (Nginx Load Balancer -> Multiple Gateway Nodes).
*   **Action**:
    1.  Receives Order.
    2.  Hashes Symbol (e.g., `BTC_USDT`) to determine **Partition ID**.
    3.  Writes Order to **Redpanda** (Partitioned by Symbol).
    4.  Waits for Redpanda ACK (Durability Guarantee).

### B. Input Log (The Sequencer)
*   **Technology**: **Redpanda** (Kafka Protocol).
*   **Role**: Durable Log & Global Sequencer per Symbol.
*   **Configuration**:
    *   **Sharding**: 1 Partition per Symbol (e.g., P0=BTC, P1=ETH).
    *   **Ordering**: Strict ordering within a partition guarantees deterministic execution.
    *   **Performance**: ~2ms latency.

### C. Matching Engine (The Core)
*   **Technology**: Rust (In-Memory).
*   **State**: OrderBook + Ledger (RAM).
*   **Logic**:
    1.  **Consume**: Reads from Redpanda (Batch Consumer).
    2.  **Execute**: Deterministic Matching (Price-Time Priority).
    3.  **Hash**: Calculates `ChainHash` (Merkle Root of State) for verification.
    4.  **Publish (Dual Path)**:
        *   **Path A (Critical)**: Sends Trades/Sequence to **Settlement Service** via ZeroMQ (Blocking/Safe).
        *   **Path B (Fast)**: Sends Tickers/Depth to **Market Data Gateway** via ZeroMQ (Non-blocking/Fast).
    5.  **Buffer**: Retains last ~100k trades in a Ring Buffer for fast re-transmission.
*   **Flow Control**: If the Ring Buffer (Path A) fills, the Engine **HALTS**. Path B drops are ignored.
*   **Data Integrity**:
    *   **Order Checksum**: **CRC32** (Standard). Calculated at Gateway. Fast, hardware-accelerated, and interoperable across languages.
        *   *Reason*: Compatibility with non-Rust gateways (Java/Go) and standard network protocols.
    *   **State Hash**: **XXHash3 (64-bit)**. Calculated at ME. Extremely fast and collision-resistant for verifying replica consistency over long chains.
        *   *Reason*: Superior speed/safety ratio compared to CRC32 (unsafe for state) or BLAKE3 (overkill).

### D. Settlement Service (The Verifier & Private Gateway)
*   **Role**: Persists trades, updates balances, and pushes **Private User Notifications**.
*   **Logic**:
    1.  **Subscribe**: Listens to ZeroMQ from ME Primary/Replica.
    2.  **Verify**: Checks `ChainHash`.
    3.  **Persist**: Writes to Database.
    4.  **Notify**: Pushes "Order Filled" / "Balance Update" events to Users (Guaranteed Delivery).
    5.  **Gap Detection**: Request re-transmission if needed.
    6.  **Periodic Save**: Save the **Last Processed Sequence ID** to **ScyllaDB/StarRocks** (e.g., `sys_checkpoints` table).
        *   *Benefit*: Decouples recovery from local disk. If the server dies, a new instance can resume instantly from the DB checkpoint.

### E. Storage Layer
*   **Hot State (User Balances)**: **ScyllaDB**.
    *   Low latency (<1ms), high concurrency.
*   **Cold/Analytics (History)**: **StarRocks**.
    *   Real-time OLAP.
    *   Handles "Order Status" updates (Mutable Primary Key) efficiently.
*   **Cold/Analytics (History)**: **StarRocks**.
    *   Real-time OLAP.
    *   Handles "Order Status" updates (Mutable Primary Key) efficiently.
    *   Stores Trade History (Immutable).

### F. Order History Service (The Query Layer)
*   **Role**: Maintains authoritative state of Active and Historic Orders.
*   **Source of Truth**: Consumes the Matching Engine's ZMQ Output Stream (Follower).
*   **Architecture Detail**: See [**ORDER_HISTORY_ARCH.md**](./ORDER_HISTORY_ARCH.md) for the full State Machine and Event Lifecycle.
*   **Logic**:
    1.  **Ingest**: Listens for Unified `OrderUpdate` events (New, Rejected, Fill, Cancel).
    2.  **Replay**: Deterministically updates order status (`NEW` -> `PARTIAL` -> `FILLED` / `CANCELLED`).
    3.  **Persist**: Writes to **ScyllaDB** (`active_orders`, `order_history`).
    4.  **Query**: Serves API requests for "Open Orders" and "Order History".
*   **Benefit**: Decouples complex queries from the Matching Engine. The ME remains pure in-memory and fast, while OHS handles state-heavy user queries.

## 2.1 Event Sourcing Strategy (The Source of Truth)
For financial integrity, we **DO NOT** rely on the current state of user balances in ScyllaDB as the source of truth. Instead, we use **Event Sourcing**.

### A. The Ledger Events Table
We maintain a complete, immutable log of all balance-changing events in ScyllaDB.

```sql
CREATE TABLE ledger_events (
    user_id bigint,
    sequence_id bigint,
    event_type text, -- 'DEPOSIT', 'WITHDRAWAL', 'TRADE_SPEND', 'TRADE_RECEIVE'
    amount bigint,
    currency int,
    related_id bigint, -- trade_id or tx_id
    PRIMARY KEY (user_id, sequence_id)
);
```

### B. Replay & Audit
*   **Replay**: To verify a user's balance, we replay `SELECT sum(amount) FROM ledger_events WHERE user_id = ?`.
*   **Audit**: If a user disputes a balance, we provide the full history of events (Deposits + Trades + Withdrawals) that led to the current state.
*   **Snapshot**: The `user_balances` table is merely a **Snapshot** (cache) of the sum of these events, optimized for fast reads. It can be rebuilt from `ledger_events` at any time.

## 3. Recovery Strategy

### Fast Restart (Process Crash)
1.  ME restarts with empty memory.
2.  ME requests "Last Processed Sequence" from Settlement Service.
3.  ME replays Redpanda Input Log from that sequence.
4.  **Optimization**: Periodic Async Snapshots (to disk) to reduce replay time.

### Disaster Recovery (Total Failure)
1.  If all MEs and Settlers fail, the **Redpanda Log** is preserved.
2.  System restarts, loads last Snapshot, and replays Redpanda to reconstruct exact state.

## 4. Key Trade-offs & Decisions

*   **Why Redpanda as Sequencer?**
    *   *Decision*: Use Redpanda directly instead of a custom RAM Sequencer.
    *   *Trade-off*: Accepts ~2ms latency for **100% Durability and Simplicity**.
    *   *Config*: Must use `acks=all` (or `min.insync.replicas=2`) to guarantee data is not lost if a broker fails.
*   **Why ZeroMQ for Output?**
    *   *Decision*: Use ZeroMQ instead of Kafka for outputs.
    *   *Benefit*: Ultra-low latency (<50µs), no disk I/O overhead. Durability is provided by the Input Log (SMR).
*   **Why Halt on Buffer Full?**
    *   *Decision*: Stop trading if Settlement is down.
    *   *Reason*: "Settlement is Critical Path". Trading without settlement creates unbacked state.
    *   *Mechanism*: Application-level check (Ring Buffer Full -> Halt), not ZMQ socket blocking.

## 4.1 Reserve Order WAL (Feature Flag)
*   **Strategy**: Keep the local Order WAL code but gate it behind a config flag (`enable_local_wal`).
*   **Default**: `false` (Pure Memory Mode).
*   **Use Case**:
    1.  **Safety Net**: Instant fallback to local durability if Redpanda becomes unstable.
    2.  **Benchmarking**: Scientific A/B testing of "Cost of Durability".
    3.  **Hybrid Deployment**: Enable for low-volume/high-value symbols if needed.

## 4.2 Operational Tuning
*   **ZeroMQ Tuning**: Set `ZMQ_RCVHWM` and `ZMQ_SNDHWM` to **1,000,000** to prevent packet drops during temporary slowdowns.
*   **Verification Logic**: Settlement Service verifies `ChainHash` **per Sequence ID**. It buffers messages from replicas until sequence alignment is achieved.


## 5. Architecture Diagram


```mermaid
graph TD
    User -->|WS/REST| LB[Load Balancer]
    LB --> GW[Gateway Cluster]

    subgraph "Input / Sequencer"
    GW -->|Produce| RP[(Redpanda)]
    end

    subgraph "Matching Engine (SMR)"
    RP -->|Consume P0| ME1[ME Node 1]
    RP -->|Consume P0| ME2[ME Node 2]
    ME1 -- Ring Buffer --> ME1
    end

    subgraph "Output / Settlement"
    ME1 -->|Path A (Critical)| SET[Settlement Service]
    ME1 -->|Path A (Critical)| OHS[Order History Service]
    ME2 -->|Path A (Critical)| SET
    SET -->|SEQ Check| ME1
    end

    subgraph "Market Data (Fast)"
    ME1 -->|Path B (Fast)| MD[Market Data GW]
    MD -->|WS Ticker| User
    end

    subgraph "Storage"
    SET -->|Balances| SCY[(ScyllaDB)]
    OHS -->|Orders| SCY
    SET -->|History| STR[(StarRocks)]
    end
```

## 6. Future Optimizations & Concerns (Roadmap)

### A. Advanced Optimizations (Phase 2)
1.  **The "Sequencer" Optimization (Ultra-Low Latency)**:
    *   *Concept*: Decouple Execution from Persistence. Use a RAM-based Sequencer to multicast orders to ME (Fast Path) and Redpanda (Slow Path) simultaneously.
    *   *Benefit*: Reduces wire-to-wire latency from ~2ms (Redpanda) to <50µs (UDP).
    *   *Risk*: "Phantom Trades" if Sequencer crashes before Redpanda write. Requires complex rollback logic or "Optimistic UI".
2.  **Kernel Bypass Networking**:
    *   *Concept*: Use **DPDK** or **Solarflare OpenOnload** to bypass the Linux Kernel TCP/IP stack.
    *   *Benefit*: Reduces OS jitter and interrupt overhead. Essential for consistent sub-10µs latency.
3.  **Sharded Settlement**:
    *   *Concept*: Partition the Settlement Service by User ID.
        *   Shard 1: Users 0-1M -> Scylla Shard 1.
        *   Shard 2: Users 1M-2M -> Scylla Shard 2.
    *   *Benefit*: Infinite horizontal scaling of the settlement layer. Eliminates the database write bottleneck.
4.  **Gateway Batching (Throughput Optimization)**:
    *   *Concept*: Gateway buffers orders for a tiny window (e.g., 50µs or 10 orders) and writes them as a single "Batch Message" to Redpanda.
    *   *Benefit*: Increases Redpanda throughput by 10x (e.g., 50k -> 500k TPS) by reducing syscalls and network overhead.
    *   *Trade-off*: Adds small latency (~50µs) to the first order in the batch. Ideal for high-load periods.
5.  **Remote S3 Snapshots (Stateless ME)**:
    *   *Concept*: Instead of saving snapshots to Local Disk, upload them to **S3/MinIO**.
    *   *Benefit*: Makes the ME truly "Stateless". A new ME instance can start on any machine, download the snapshot from S3, and resume.
    *   *Note*: This applies to the **Matching Engine** (large state). The **Settlement Service** simply stores its `Last_Seq_ID` in **ScyllaDB** (tiny state, frequent updates).
    *   *Current State*: We use Local Disk Snapshots for simplicity and speed.

### B. Known Concerns & Risks
1.  **Redpanda Tail Latency**:
    *   *Risk*: If Redpanda experiences a GC pause or disk stall, the entire exchange stalls (since ME waits for Input).
    *   *Mitigation*: Use NVMe RAID, tune Seastar reactor, and isolate Redpanda on dedicated cores.
2.  **ZeroMQ "Death Spiral"**:
    *   *Risk*: If a subscriber is slow, the Publisher might block or drop packets, causing a cascade of Replay Requests.
    *   *Mitigation*: Strict monitoring of Ring Buffer depth. Aggressive HWM settings. Rate-limit replay requests.
3.  **Single Point of Failure (SPOF) - Sequencer**:
    *   *Risk*: If we move to a custom Sequencer, it becomes a SPOF.
    *   *Mitigation*: Active-Passive Failover with VIP/Etcd (adds complexity).

## 7. Kafka/Redpanda Configuration & Warnings

### A. Critical Settings (Must Have)
1.  **Durability (The "Bank Vault" Mode)**:
    *   `acks=all` (or `-1`): Ensures leader AND followers acknowledge the write.
    *   `min.insync.replicas=2` (for RF=3): Ensures at least 2 copies exist before confirming to Gateway.
    *   *Warning*: Setting `acks=1` improves latency (0.5ms vs 2ms) but **risks data loss** if the leader crashes. For SMR, data loss = state corruption. **DO NOT USE acks=1**.
2.  **Ordering (The "Sequencer" Mode)**:
    *   `enable.idempotence=true`: Prevents duplicate messages during retries.
    *   `max.in.flight.requests.per.connection=1` (if idempotence is false): Strictly enforces order. With idempotence=true, this can be 5.
    *   *Warning*: Never use a producer that retries without idempotence, or you will get duplicate trades.

### B. Latency Tuning (The "Race Car" Mode)
1.  **Producer**:
    *   `linger.ms=0`: Send immediately, do not wait for batching (for lowest latency).
    *   `compression.type=none` (or `lz4`): Compression adds CPU latency. For small orders, `none` is faster.
2.  **Consumer (Matching Engine)**:
    *   `fetch.min.bytes=1`: Return data as soon as 1 byte is available.
    *   `socket.receive.buffer.bytes`: Increase to 1MB+ to avoid kernel-level drops.

### C. Operational Warnings
1.  **Partition Count**:
    *   **Rule**: 1 Partition per Symbol.
    *   *Warning*: **NEVER** increase partitions for an existing symbol (e.g., BTC_USDT). This breaks total ordering and destroys determinism.
2.  **Log Compaction**:
    *   **Rule**: Use `cleanup.policy=delete` (Retention).
    *   *Warning*: **DO NOT** use `compact` for the Order Log. You need the *sequence* of events, not just the latest state. Compaction removes history needed for replay.
3.  **Retention Policy**:
    *   Set `retention.bytes` or `retention.ms` high enough to cover the time needed to restore from a Snapshot (e.g., 7 days). If you lose the tail of the log before taking a snapshot, you cannot recover.
4.  **Determinism Audit**:
    *   **Rule**: The Matching Engine logic MUST be deterministic.
    *   *Warning*: **NEVER** use `SystemTime::now()`, `Random()`, or thread scheduling inside the matching loop. Use the `timestamp` from the Input Log (Redpanda) for all time-based logic.

## 8. Deployment & Storage Strategy (Tiered Storage)

### A. Redpanda Tiered Storage (Infinite History)
*   **Concept**: Use S3 (or MinIO) as the "Cold Store" for the Input Log.
*   **Configuration**:
    *   `cloud_storage_enabled=true`
    *   `retention.local.target.bytes`: Set to ~100GB (fits on NVMe).
    *   `retention.bytes`: Set to `-1` (Infinite) for S3.
*   **Benefit**: Keeps local disk lean and fast. Allows "Replay from Genesis" for audit/debugging without filling local storage.

### B. Snapshot & Prune (Fast Recovery)
*   **Concept**: "Checkpoints" allow the ME to skip replaying old history.
*   **Protocol**:
    1.  **Trigger**: Every N trades (e.g., 1M) or T time (e.g., 1 hour).
    2.  **Snapshot**: ME takes an async snapshot of RAM state (OrderBook + Ledger) and uploads to S3 (e.g., `snapshot_seq_1000000.bin`).
    3.  **Checkpoint**: ME updates `sys_checkpoints` DB table: `LastSnapshot = 1,000,000`.
*   **Recovery Flow**:
    1.  ME boots up.
    2.  Reads `LastSnapshot` -> 1,000,000.
    3.  Downloads `snapshot_seq_1000000.bin` from S3 and loads into RAM.
    4.  Asks Redpanda for logs starting from **1,000,001**.
    5.  **Result**: Instant startup (seconds) vs Replay (hours).

## 9. Observability & Monitoring (The Dashboard)

### A. Golden Signals (Must Alert)
1.  **Sequence Lag**:
    *   *Metric*: `Current_Seq_ID (ME) - Committed_Seq_ID (Settler)`
    *   *Threshold*: `> 1000` (Warning), `> 10000` (Critical).
    *   *Meaning*: Settlement is falling behind. Risk of buffer overflow.
2.  **End-to-End Latency**:
    *   *Metric*: `Settlement_Time - Order_Creation_Time`
    *   *Threshold*: `> 10ms` (Warning).
    *   *Meaning*: System is slow. User experience degrading.
3.  **Data Integrity Failures**:
    *   *Metric*: `Checksum_Mismatch_Count` OR `ChainHash_Mismatch_Count`
    *   *Threshold*: `> 0` (CRITICAL P0).
    *   *Meaning*: Data corruption or non-deterministic execution. **HALT IMMEDIATELY**.
4.  **Buffer Saturation**:
    *   *Metric*: `Ring_Buffer_Usage_Percent`
    *   *Threshold*: `> 80%` (Warning).
    *   *Meaning*: Approaching "Stop the World" condition. Scale up Settlers.

### B. Operational Metrics
1.  **Redpanda Lag**: Consumer Group Lag for Matching Engine.
2.  **ZeroMQ Drops**: `ZMQ_EVENTS_DROPPED` count (on Market Data channel).
3.  **Snapshot Age**: Time since last successful snapshot (Alert if > 2 hours).

## 10. Security & Compliance (Financial Grade)

### A. Network Security
*   **ZeroMQ Risk**: ZeroMQ (PUB/SUB) has no built-in authentication by default.
*   **Mandate**: All ME-to-Settlement traffic MUST run inside a **Private VPC** (Virtual Private Cloud).
*   **Firewall**: Block port 5557/5558 from public internet. Only allow whitelisted internal IPs.

### B. Audit & Immutability
*   **WORM Storage**: Configure S3 Bucket for Redpanda Tiered Storage with **Object Lock (WORM)** enabled.
    *   *Benefit*: Prevents "History Rewriting" attacks. Even root cannot delete old trade logs for N years.
*   **Checksum Verification**: The Settlement Service MUST verify the `Order Checksum` before writing to the DB to prove the data wasn't tampered with in transit.
