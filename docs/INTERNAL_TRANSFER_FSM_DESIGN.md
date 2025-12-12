# Internal Transfer State Machine Design
**Objective:** Formalize the Internal Transfer lifecycle using `rust-fsm` to ensure type-safe, deterministic state transitions.

## 1. Current State (Implicit)
Currently, the system uses string-based or simple enum-based status updates (`Requesting`, `Pending`, `Success`, `Failed`). Transitions are handled via ad-hoc logic in `InternalTransferHandler` and `InternalTransferSettlement`.

## 2. Proposed FSM Design

We will define a `InternalTransferStateMachine` using `rust-fsm`.

### States
1.  **Requesting**: Initial state when API receives request.
2.  **Processing** (formerly implicit "processing_ubs"): Logic is being verified or routed (e.g. sent to Kafka).
3.  **Pending**: Funds are effectively locked (TigerBeetle PENDING transfer created).
4.  **Success**: Terminal state. Transfer finalized/posted.
5.  **Failed**: Terminal state. Transfer rejected/voided.

### Events (Inputs)
1.  **Submit**: API validation passed, start processing.
2.  **RouteToUbs**: For Spot->Funding, route to UBSCore.
3.  **LockFunds**: TigerBeetle `create_transfers` (Pending) successful.
4.  **Settle**: Final settlement confirmed (TigerBeetle `post_transfer`).
5.  **Fail**: Error occurred (Balance insufficient, Timeout, etc).

### State Transition Table

| Source State | Event | Target State | Action |
| :--- | :--- | :--- | :--- |
| **Requesting** | `Submit` | **Processing** | Validate request |
| **Processing** | `RouteToUbs` | **Processing** | Publish to Kafka (Spot->Funding) |
| **Processing** | `LockFunds` | **Pending** | Create PENDING transfer (Funding->Spot) |
| **Pending** | `Settle` | **Success** | Post Transfer |
| **Pending** | `Fail` | **Failed** | Void Transfer |
| **Requesting** | `Fail` | **Failed** | Return Error |
| **Processing** | `Fail` | **Failed** | Return Error |

## 3. Implementation Plan
1.  **Add Dependency**: `cargo add rust-fsm`.
2.  **Define FSM**: Create `src/models/internal_transfer_fsm.rs`.
3.  **Integrate**:
    *   Update `InternalTransferHandler` to use FSM for state transitions.
    *   Update `InternalTransferSettlement` to drive the FSM from `Pending` -> `Success`.
    *   Map FSM states to the existing `TransferStatus` enum (or replace it).

### Rust Code Preview

```rust
use rust_fsm::state_machine;

state_machine! {
    derive(Debug, Clone, PartialEq)
    InternalTransfer(Requesting)

    Requesting => {
        Submit => Processing,
        Fail => Failed,
    },
    Processing => {
        RouteToUbs => Processing, // Self-transition (side effect) or distinct state?
        LockFunds => Pending,
        Fail => Failed,
    },
    Pending => {
        Settle => Success,
        Fail => Failed,
    }
}
```

## 4. Considerations
*   **Persistence**: The FSM state is transient in memory but mapped to ScyllaDB `status` column. We will implement `FromStr` / `ToString` for persistence.
*   **Side Effects**: Actions like "Publish to Kafka" or "Call TigerBeetle" can be triggered by state entry/exit or handled by the driver.

## 5. Implementation Status
- **FSM Implementation**: Implemented a custom Rust-native State Machine (due to `rust-fsm` macro limitations). Located in `src/models/internal_transfer_fsm.rs`.
- **Integration**:
    - `InternalTransferHandler`: Fully integrated. Uses FSM for Requesting -> Processing -> Pending.
    - `InternalTransferSettlement`: Fully integrated. Uses FSM for Pending -> Success/Failed.
- **Verification**:
    - `tests/13_spot_funding_e2e.sh`: Verified E2E flow (Pending -> Success).
    - `tests/14_rescue_transfer_e2e.sh`: Verified recovery flow.
