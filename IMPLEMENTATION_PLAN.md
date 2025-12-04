# Implementation Plan: Pure Memory SMR Exchange (Granular)

This plan is designed for **Atomic Development Sessions**. Each task is small enough to be completed, verified, and committed in a single interaction.

**⚠️ PROTOCOL RULE**: Before starting any task, you MUST update `AI_STATE.yaml` to set the task status to `IN_PROGRESS`. This ensures resumption capability.

## Phase 1: Pure Memory Core (The Cleanup)

### Task 1.1: Configurable WAL (Feature Flag)
*   **Goal**: Make the local WAL optional without breaking existing code.
*   **Steps**:
    1.  [ ] **Modify**: Add `enable_local_wal: bool` to `AppConfig` struct and `config.yaml`.
    2.  [ ] **Modify**: Update `matching_engine_server.rs` to read this flag.
    3.  [ ] **Modify**: Wrap `OrderWal::new()` and `wal.log_place_order()` calls in `if config.enable_local_wal { ... }`.
    4.  [ ] **Check**: Run `cargo check`.
    5.  [ ] **Verify**: Run the server with `enable_local_wal: false`. Send an order. Ensure no WAL files are created.
    6.  [ ] **Commit**: `git commit -m "feat: add enable_local_wal feature flag"`

### Task 1.2: Verify Pure Memory Replication
*   **Goal**: Prove that SMR works without the WAL.
*   **Steps**:
    1.  [ ] **Modify**: Update `test_replication.sh` to ensure `enable_local_wal` is `false` for both nodes.
    2.  [ ] **Verify**: Run `test_replication.sh`.
    3.  [ ] **Check**: Confirm both nodes output identical logs and process the same number of trades.
    4.  [ ] **Commit**: `git commit -m "test: verify pure memory replication"`

## Phase 2: The Output Pipeline (ZeroMQ & Hashing)

### Task 2.1: Add Dependencies
*   **Goal**: Prepare the environment.
*   **Steps**:
    1.  [ ] **Modify**: Add `zeromq = "0.4"` and `xxhash-rust = { version = "0.8", features = ["xxh3"] }` to `Cargo.toml`.
    2.  [ ] **Check**: Run `cargo build` to ensure dependencies compile.
    3.  [ ] **Commit**: `git commit -m "chore: add zeromq and xxhash dependencies"`

### Task 2.2: Implement State Hashing
*   **Goal**: Calculate deterministic state hash.
*   **Steps**:
    1.  [ ] **Modify**: Add `state_hash: u64` to `MatchingEngine` struct.
    2.  [ ] **Modify**: Implement `update_hash(&mut self, data: &[u8])` using `xxh3_64`.
    3.  [ ] **Modify**: Call `update_hash` inside `process_order`.
    4.  [ ] **UnitTest**: Write a test case: Process Order A -> Check Hash. Process Order A again -> Check Hash. Must be identical.
    5.  [ ] **Commit**: `git commit -m "feat: implement xxhash3 state hashing"`

### Task 2.3: ZeroMQ Publisher Setup
*   **Goal**: Create the output channel.
*   **Steps**:
    1.  [ ] **Modify**: Create `src/publisher.rs`. Implement a `ZmqPublisher` struct.
    2.  [ ] **Modify**: Initialize `ZmqPublisher` in `matching_engine_server.rs`.
    3.  [ ] **Verify**: Run a simple Python script to subscribe to the ZMQ port and print messages.
    4.  [ ] **Commit**: `git commit -m "feat: setup zeromq publisher"`

## Phase 3: Settlement Service (The Verifier)

### Task 3.1: Skeleton Service
*   **Goal**: A minimal service that listens.
*   **Steps**:
    1.  [ ] **Modify**: Create `src/bin/settlement_service.rs`.
    2.  [ ] **Modify**: Implement ZMQ Subscriber loop.
    3.  [ ] **Verify**: Run ME (Publisher) and Settlement (Subscriber). Confirm messages flow.
    4.  [ ] **Commit**: `git commit -m "feat: create skeleton settlement service"`

### Task 3.2: Typed Deserialization
*   **Goal**: Convert raw JSON into strong Rust types.
*   **Steps**:
    1.  [ ] **Modify**: Define `SettlementEntry` struct in `src/models.rs` (or locally if preferred) to match ME output.
    2.  [ ] **Modify**: Update `settlement_service.rs` to deserialize into this struct.
    3.  [ ] **Verify**: Run and confirm structured logging works.
    4.  [ ] **Commit**: `git commit -m "feat: implement settlement deserialization"`

### Task 3.3: Sequence Verification
*   **Goal**: Ensure no data loss (Gap Detection).
*   **Steps**:
    1.  [ ] **Modify**: Add `next_sequence: u64` state to `settlement_service.rs`.
    2.  [ ] **Modify**: Implement check: `if msg.sequence != next_sequence { panic/log }`.
    3.  [ ] **Verify**: Run normal flow (should pass). Manually inject a gap (optional) to test failure.
    4.  [ ] **Commit**: `git commit -m "feat: implement settlement sequence verification"`

### Task 3.4: Simple Persistence (CSV)
*   **Goal**: Persist trades to disk (simulating DB).
*   **Steps**:
    1.  [x] **Modify**: Add `csv` crate dependency.
    2.  [x] **Modify**: Create a `TradeWriter` in `settlement_service.rs`.
    3.  [x] **Modify**: Append settled trades to `settled_trades.csv`.
    4.  [x] **Verify**: Run E2E test and check `settled_trades.csv` content.
    5.  [x] **Commit**: `git commit -m "feat: implement settlement csv persistence"`

## Phase 4: Database Integration (ScyllaDB)

### Task 4.1: ScyllaDB Setup & Configuration
*   **Goal**: Prepare ScyllaDB environment and configuration.
*   **Steps**:
    1.  [x] **Modify**: Add `scylla` crate dependency to `Cargo.toml`.
    2.  [x] **Modify**: Add ScyllaDB configuration to `config.yaml` (hosts, keyspace, replication).
    3.  [x] **Create**: `docker-compose.yml` entry for ScyllaDB (single node for dev).
    4.  [x] **Verify**: Run `docker-compose config` and confirm it's valid.
    5.  [x] **Commit**: `git commit -m "chore: add scylladb setup and configuration"`

### Task 4.2: Schema Definition & Migration
*   **Goal**: Create the settlement database schema.
*   **Steps**:
    1.  [x] **Create**: `schema/settlement_schema.cql` with keyspace and table definitions.
    2.  [x] **Design**: Table `settled_trades` with columns: trade_id, sequence, buyer_id, seller_id, price, quantity, timestamp, etc.
    3.  [x] **Design**: Add appropriate partition key (trade_date) and clustering key (output_sequence).
    4.  [x] **Create**: `scripts/init_scylla.sh` to apply schema.
    5.  [x] **Verify**: Schema created and script is executable.
    6.  [x] **Commit**: `git commit -m "feat: add scylladb settlement schema"`

### Task 4.3: Database Client Abstraction
*   **Goal**: Create a clean abstraction for database operations.
*   **Steps**:
    1.  [x] **Create**: `src/db/settlement_db.rs` module.
    2.  [x] **Implement**: `SettlementDb` struct with ScyllaDB session.
    3.  [x] **Implement**: `async fn connect(config: &DbConfig) -> Result<SettlementDb>`.
    4.  [x] **Implement**: `async fn insert_trade(&self, trade: &MatchExecData) -> Result<()>`.
    5.  [x] **UnitTest**: Tests added (ignored, require running ScyllaDB).
    6.  [x] **Commit**: `git commit -m "feat: implement settlement database client"`

### Task 4.4: Integrate Database into Settlement Service
*   **Goal**: Replace/augment CSV with ScyllaDB persistence.
*   **Steps**:
    1.  [x] **Modify**: Update `settlement_service.rs` to initialize `SettlementDb`.
    2.  [x] **Modify**: Make main loop async (use `tokio::main`).
    3.  [x] **Modify**: Call `db.insert_trade(&trade).await` after receiving each trade.
    4.  [x] **Modify**: Keep CSV writing as backup/audit trail (dual write).
    5.  [x] **Verify**: Compiles successfully.
    6.  [x] **Commit**: `git commit -m "feat: integrate scylladb into settlement service"`

### Task 4.5: Batch Insertion Optimization (DEFERRED)
*   **Goal**: Improve throughput with batch inserts.
*   **Status**: Deferred per user request ("leave opt later").
*   **Steps**:
    1.  [ ] **Modify**: Implement `async fn insert_batch(&self, trades: &[MatchExecData]) -> Result<()>`.
    2.  [ ] **Modify**: Buffer trades in settlement service (e.g., 100 trades or 100ms timeout).
    3.  [ ] **Modify**: Use ScyllaDB prepared statements for batch inserts.
    4.  [ ] **Verify**: Measure throughput improvement (log batch size and latency).
    5.  [ ] **Commit**: `git commit -m "feat: implement batch insertion for settlement"`

### Task 4.6: Query Interface & Verification
*   **Goal**: Add read operations to verify settlement data.
*   **Steps**:
    1.  [x] **Implement**: `get_trade_by_id` in `SettlementDb`.
    2.  [x] **Implement**: `get_trades_by_sequence_range` in `SettlementDb`.
    3.  [x] **Create**: `verify_settlement` CLI tool for verification.
    4.  [x] **Verify**: Run verification tool against populated DB.
    5.  [x] **Commit**: `git commit -m "feat: add settlement query interface and verification tool"`

### Task 4.7: Error Handling & Retry Logic
*   **Goal**: Make settlement service resilient to database failures.
*   **Steps**:
    1.  [ ] **Modify**: Implement retry logic for transient ScyllaDB errors.
    2.  [ ] **Modify**: Add exponential backoff for connection failures.
    3.  [ ] **Modify**: Log failed trades to a dead-letter file for manual recovery.
    4.  [ ] **Verify**: Simulate ScyllaDB downtime, confirm service recovers gracefully.
    5.  [ ] **Commit**: `git commit -m "feat: add error handling and retry logic"`

### Task 4.8: Monitoring & Metrics
*   **Goal**: Add observability for settlement database operations.
*   **Steps**:
    1.  [ ] **Modify**: Add metrics: `settlement_db_inserts_total`, `settlement_db_latency_ms`, `settlement_db_errors_total`.
    2.  [ ] **Modify**: Log slow queries (> 100ms).
    3.  [ ] **Modify**: Add health check endpoint that verifies ScyllaDB connectivity.
    4.  [ ] **Verify**: Run load test and observe metrics.
    5.  [ ] **Commit**: `git commit -m "feat: add settlement metrics"`

### Task 4.9: Event Sourcing Implementation
*   **Goal**: Implement complete audit trail for all balance changes.
*   **Steps**:
    1.  [ ] **Schema**: Add `ledger_events` table to `settlement_schema.cql`.
    2.  [ ] **Modify**: Update `SettlementDb` to support `insert_ledger_event`.
    3.  [ ] **Modify**: Update `settlement_service.rs` to handle Deposit/Withdrawal messages.
    4.  [ ] **Verify**: Verify deposits/withdrawals are persisted to ScyllaDB.
    5.  [ ] **Commit**: `git commit -m "feat: implement event sourcing for settlement"`

## Phase 5: Advanced Features (Future)

### Task 5.1: StarRocks Integration (Analytics)
*   **Goal**: Add real-time analytics database for settlement data.
*   **Steps**: *(To be defined)*

### Task 5.2: S3 Archival
*   **Goal**: Archive old settlement data to S3 for compliance.
*   **Steps**: *(To be defined)*

### Task 5.3: Settlement Reconciliation
*   **Goal**: Periodic reconciliation between ME state and settlement DB.
*   **Steps**: *(To be defined)*

