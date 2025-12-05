
# Order History Implementation Plan

**Objective**: Implement full Order Lifecycle tracking (Active & Historic) using granular, atomic steps.
**Architecture**: Defined in `ORDER_HISTORY_ARCH.md`.

## Phase 1: Core Domain Models
**Goal**: Define the "Language" of Order Updates.

### Step 1.1: Define OrderStatus & OrderUpdate
- **Action**: Modify `src/ledger.rs`.
  - Add `enum OrderStatus { New, PartiallyFilled, Filled, Cancelled, Rejected, Expired }`.
  - Add `struct OrderUpdate` (ExecutionReport) with fields: `order_id`, `client_order_id` (optional), `user_id`, `symbol`, `status`, `price`, `qty`, `filled_qty`, `avg_fill_price` (optional), `timestamp`.
- **Verification**:
  - `cargo check` to ensure syntax is correct.
  - **Unit Test**: Create `tests/models_test.rs` (or add to `src/ledger.rs` tests module) to verify serialization/deserialization of `OrderUpdate` and `OrderStatus` (JSON & Bincode).
- **Commit**: "feat(models): define OrderStatus and OrderUpdate"

## Phase 2: Matching Engine Instrumentation (The Emitter)
**Goal**: Make the Matching Engine emit lifecycle events.

### Step 2.1: Extend LedgerCommand
- **Action**: Modify `src/ledger.rs`.
  - Add `OrderUpdate(OrderUpdate)` variant to `LedgerCommand` enum.
- **Verification**:
  - `cargo check`. Expect warnings/errors where `LedgerCommand` is matched.
  - **Unit Test**: Ensure `LedgerCommand` with `OrderUpdate` variant can be serialized/deserialized seamlessly in existing tests.
- **Commit**: "feat(ledger): add OrderUpdate variant to LedgerCommand"

### Step 2.2: Instrument Order Entry (New/Rejected)
- **Action**: Modify `src/reconciliation.rs` or `matching_engine_base.rs`.
  - Inside `process_order`:
    - On Validation Failure: Emit `OrderUpdate { status: Rejected }`.
    - On Book Add Success: Emit `OrderUpdate { status: New }`.
- **Verification**:
  - **Unit Test**: Add a test in `matching_engine_base.rs` (e.g., `test_order_lifecycle_emission`) that places an order and asserts that `OrderUpdate(New)` is found in the returned `LedgerCommand` list.
  - **Unit Test**: Add a test for Insufficient Funds and assert `OrderUpdate(Rejected)`.
- **Commit**: "feat(me): emit OrderUpdate for New and Rejected orders"

### Step 2.3: Instrument Cancellations
- **Action**: Modify `matching_engine_base.rs` -> `cancel_order`.
  - On Success: Emit `OrderUpdate { status: Cancelled }`.
- **Verification**:
  - **Unit Test**: Add a test that cancels an order and asserts `OrderUpdate(Cancelled)` is emitted.
- **Commit**: "feat(me): emit OrderUpdate for Cancellations"

## Phase 3: Infrastructure & Service Scaffold
**Goal**: Prepare the Consumer (Order History Service).

### Step 3.1: Database Schema
- **Action**: Create `schema/order_history.cql` or update `init.cql`.
  - Table `active_orders` (Partition: `user_id`).
  - Table `order_history` (Partition: `user_id`, Cluster: `created_at` DESC).
- **Verification**:
  - `docker-compose up -d scylla` and checks logs / query system tables.
  - **Test**: Run a CQL script to insert dummy data and read it back to verify schema correctness.
- **Commit**: "feat(db): add order history tables"

### Step 3.2: Service Skeleton & ZMQ Consumer
- **Action**: Implement `src/bin/order_history_service.rs`.
  - Setup `tokio` main.
  - Setup ZMQ `PULL` socket connecting to ME's settlement address.
  - Loop and deserialize `LedgerCommand`. Print received commands to stdout.
- **Verification**:
  - Run `cargo run --bin matching_engine`.
  - Run `cargo run --bin order_history_service`.
  - Send an order, verify OHS prints the `OrderUpdate` event.
- **Commit**: "feat(ohs): init service with ZMQ consumer"

## Phase 4: Data Persistence & Logic
**Goal**: Persist state to Scylla.

### Step 4.1: Scylla Integration
- **Action**: Update `order_history_service.rs`.
  - Initialize `scylla::Session`.
  - Implement `ActiveOrdersRepository::upsert` and `delete`.
  - Implement `OrderHistoryRepository::insert`.
- **Verification**:
  - **Integration Test**: Write `tests/order_history_db_test.rs` that explicitly calls upsert/insert/delete against a running Scylla instance and asserts table state.
- **Commit**: "feat(ohs): add Scylla repositories"

### Step 4.2: Event Processing Logic
- **Action**: Update `order_history_service.rs` loop.
  - Match `LedgerCommand::OrderUpdate(u)`:
    - `New` -> Upsert Active, Insert History.
    - `Rejected` -> Insert History.
    - `Cancelled` -> Delete Active, Insert History.
  - Match `LedgerCommand::MatchExec(m)`:
    - Update Active (filled_qty), Insert History (Fill).
    - If fully filled -> Delete Active (or mark filled).
- **Verification**:
  - **Unit Test**: Extract logic into `OrderProcessor` struct and write pure unit tests mocking the DB (or check effect on internal state) for each event type.
- **Commit**: "feat(ohs): implement event processing logic"

## Phase 5: Verification (E2E)
**Goal**: Prove it works.

### Step 5.1: End-to-End Test
- **Action**: Create `scripts/test_order_history_e2e.sh`.
  - 1. Place Order.
  - 2. Check OHS HTTP API (or query DB directly if API not ready) -> Should be OPEN.
  - 3. Cancel Order.
  - 4. Check OHS -> Should be CANCELLED in history, gone from active (or Empty list).
- **Verification**: Run script, ensure green.
- **Commit**: "test: add order history E2E verification"
