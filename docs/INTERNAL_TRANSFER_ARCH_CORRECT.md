# Internal Transfer Architecture: Correctness Analysis

## current State
The current implementation utilizes **TigerBeetle (TB)** as the settlement ledger for *both* Funding and Spot accounts.
*   **Funding -> Spot**: Works correctly. TB acts as the source of truth for Funding Wallets. Funds are locked in TB and settled to the Spot account in TB.
*   **Spot -> Funding**: The current implementation **assumes TB holds the authoritative Spot balance**. It attempts to lock funds in TB (`create_pending_transfer`).

## ⚠️ The "Spot Account" Problem
In the production architecture, **Spot Accounts are held by UBSCore (Matching Engine)** to ensure high-performance trading and manage open order locks (`available` vs `locked` balance).

**Issue**:
If a user initiates a `Spot -> Funding` transfer, referencing **TigerBeetle** for the available balance is unsafe because TB may not be aware of:
1.  Open orders in the matching engine (which reduce `available` balance).
2.  Unsettled trades not yet flushed to TB.

**Result**:
A user could theoretically withdraw funds that are currently locking an open order, leading to double-spending or negative balances in the Trading Engine.

## ✅ Correct Architecture (Future)

To correctly handle `Spot -> Funding` transfers:

1.  **Request Initiation**:
    *   User requests transfer (Spot -> Funding).
    *   Gateway validates request format.

2.  **Trading Engine Lock (UBSCore)**:
    *   Gateway sends a **Hold Request** (or `InternalOrder`) to UBSCore via Kafka/Aeron.
    *   UBSCore checks `available` balance (subtracting open orders).
    *   If sufficient, UBSCore:
        *   Decrements `available` balance.
        *   Publishes a `BalanceLocked` event.

3.  **Settlement (TigerBeetle)**:
    *   Settlement Service consumes `BalanceLocked`.
    *   Settlement Service instructs TigerBeetle to credit the Funding Account (mirroring the decrement in Spot).
    *   *Note*: Since funds are moving *out* of the Trading Engine, TB acts as the recipient ledger.

## Current MVP Limitation
The current system implements the **Settlement Layer** logic. It assumes that for the purpose of the MVP, the "Spot Balance" in TigerBeetle is the tradeable balance. This allows end-to-end testing of the transfer state machine (Request -> Pending -> Posted) but skips the complex synchrony with the Matching Engine's in-memory state.

**Next Immediate Step**:
Integrate `InternalTransferHandler` with `UBSCore`'s input stream to request checks before touching TigerBeetle for Spot-sourced transfers.
