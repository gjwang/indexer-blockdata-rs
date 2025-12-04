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
    1.  [ ] **Modify**: Add `csv` crate dependency.
    2.  [ ] **Modify**: Create a `TradeWriter` in `settlement_service.rs`.
    3.  [ ] **Modify**: Append settled trades to `settled_trades.csv`.
    4.  [ ] **Verify**: Run E2E test and check `settled_trades.csv` content.
    5.  [ ] **Commit**: `git commit -m "feat: implement settlement csv persistence"`

*(Phase 4: Database Integration - Scylla/StarRocks - to follow)*

