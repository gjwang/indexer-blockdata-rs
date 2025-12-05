# Order Status Logic & Testing Update

## Key Changes
1.  **Event Sequencing Fix**:
    -   `OrderUpdate(New)` is now emitted **strictly before** any `MatchExecBatch` events.
    -   Achieved by integrating event emission directly into the `Ledger` transaction flow within `process_order_logic`.

2.  **Architecture Refactoring**:
    -   Removed the "side-channel" `Vec<LedgerCommand>` return from `process_order_logic`.
    -   All state changes (Lock, New, Match, Cancelled) are now applied via `ledger.apply(...)`.
    -   All `OrderUpdate` events are persisted in the WAL.

3.  **Cancellation consistency**:
    -   `process_cancel` now applies `OrderUpdate(Cancelled)` to the ledger, ensuring it appears in the WAL and maintains sequence.

4.  **Tests Updated**:
    -   `matching_engine_balance_tests.rs`: Updated to expect `Ok` (with Rejected event) for insufficient funds, reflecting the event-driven architecture.
    -   `order_lifecycle_integration_tests.rs`: Similarly updated to expect `Ok` on rejection.
    -   `matching_engine_order_status_tests.rs`: Added comprehensive status transition tests.

## Status
✅ All unit and integration tests passed (`cargo test`).
System is robust against sequencing bugs.
