# UBSCore + TigerBeetle Shadow Ledger Architecture

## 1. Core Philosophy

**UBSCore is the Source of Truth.**
- In-memory, single-threaded, µs latency.
- Makes ALL business decisions (balances, matching, risk).
- Uses a WAL for durability.

**TigerBeetle is the Shadow Ledger.**
- Durable, disk-based, high-throughput.
- **Mirrors** UBSCore state exactly.
- Acts as a **Cryptographie Auditor**: It validates every balance change.
- If TigerBeetle rejects a transaction that UBSCore allowed, **IT IS A CRITICAL BUG**. The system must HALT (panic).

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          SYSTEM ARCHITECTURE                                │
└─────────────────────────────────────────────────────────────────────────────┘

       UBSCore (Leader)                    TigerBeetle (Follower/Auditor)
    ┌────────────────────┐               ┌────────────────────────────────┐
    │  In-Memory State   │    Async      │    Durable State               │
    │  (Avail / Frozen)  │ ────────────▶ │    (Posted / Pending)          │
    │                    │    Output     │                                │
    │  Decides:          │               │    Validates:                  │
    │   - Order Entry    │   Syncs:      │     - Conservation of Money    │
    │   - Matching       │   - Locks     │     - Atomic Execution         │
    │   - Risk           │   - Trades    │     - Negative Balance Check   │
    └────────────────────┘               └────────────────────────────────┘
             │                                         │
             ▼                                         ▼
        Output Stream                            Query Layer
       (Kafka / Log)                           (Gateway API)
```

---

## 2. Global Account Definition

We define a strict account hierarchy to organize user funds, pending holds, and platform revenue.

### Account ID Mapping
ID is `u128`, composed of `User ID (u64)` and `Asset ID (u32)`.

| Account Type | ID Pattern | Description |
|--------------|------------|-------------|
| **User** | `UserID | AssetID` | Standard user balances. |
| **Omnibus** | `0xFF..FF | AssetID` | Exchange cold wallet (Liabilities). |
| **Holding** | `0xFE..FE | AssetID` | Intermediate hold for pending orders. |
| **Revenue** | `0xEE..EE | AssetID` | Platform fee collection (Equity). |

*Note: Every Asset ID (BTC, USDT, ETH) has its own corresponding Omnibus, Holding, and Revenue account.*

---

## 3. Atomic Settlement Protocol

We use TigerBeetle's `LINKED` flag to ensure **All-or-Nothing** execution of trades. A trade is not just asset swapping; it includes unfreezing funds and paying fees.

**The Golden Rule**: If *any* part of the chain fails (e.g., fee payment fails), the *entire* trade rolls back.

### The Settlement Batch (Vector of Transfers)

For a trade where Buyer pays USDT and Seller pays BTC:

| Idx | Operation | From | To | Asset | Flags | Description |
|-----|-----------|------|----|-------|-------|-------------|
| 1 | **POST Buyer** | - | - | USDT | `LINKED` | Commit/Unfreeze Buyer's USDT hold. |
| 2 | **POST Seller** | - | - | BTC | `LINKED` | Commit/Unfreeze Seller's BTC hold. |
| 3 | **Principal** | Seller | Buyer | BTC | `LINKED` | Move BTC net amount. |
| 4 | **Principal** | Buyer | Seller | USDT | `LINKED` | Move USDT net amount. |
| 5 | **Fee (Buyer)** | Buyer | Revenue | USDT | `LINKED` | Pay Platform Fee. |
| 6 | **Fee (Seller)**| Seller | Revenue | USDT | `NONE` | Pay Platform Fee. **(Closes Chain)** |

*Note: The last transfer MUST NOT have the `LINKED` flag. It anchors the atomic chain.*

---

## 4. Fee Collection Architecture

We implement a **Double-Entry Fee System**. Fees are not hidden; they are explicit transfers to Revenue Accounts.

- **Granularity**: Fees are collected in the native asset of the trade or quote currency.
- **Sweeping**: A separate process sweeps `Revenue Accounts` to cold wallets periodically.
- **Audit**: `Sum(User Balances) + Sum(Revenue) == Omnibus Balance`.

---

## 5. Synchronization Logic

### The "Fire-and-Forget" Pattern with Fatal Feedback

1.  **UBSCore** executes logic in-memory. Updates `avail/frozen` instantly.
2.  **UBSCore** emits an event to the `TigerBeetleSync` queue (channel).
3.  **Background Worker** batches these events and sends to TigerBeetle.
4.  **Feedback Loop**:
    *   `Ok` -> Validation Passed.
    *   `Err(ExceedsCredits)` -> **PANIC!** UBSCore allowed money it didn't have.
    *   `Err(Connectivity)` -> Retry/Alert.

### Event Mapping Table

| UBS Operation | TigerBeetle Operation |
|---------------|-----------------------|
| `deposit()` | Transfer: `Omnibus` -> `User` |
| `withdraw()` | Transfer: `User` -> `Omnibus` |
| `lock_funds()` | Transfer (PENDING): `User` -> `Holding` |
| `unlock_funds()`| Transfer (VOID): Void the Pending Transfer |
| `settle_trade()`| **Atomic Batch** (See Section 3) |

---

## 6. Recovery Strategy

In the event of a UBSCore crash:

1.  **Boot**: UBSCore starts with empty memory.
2.  **Snapshot Load**: Query TigerBeetle for ALL user balances.
    *   `UBS.avail` = `TB.total - TB.pending`
    *   `UBS.frozen` = `TB.pending`
3.  **WAL Replay**: Replay recent operations from WAL that might not have synced to TB yet (using Kafka offsets or Sequence IDs).
4.  **Ready**: System opens for trading.

---

*Verified by Senior Architect*
*Implementation Status: Implemented (Core Integration Complete via tigerbeetle-unofficial)*
