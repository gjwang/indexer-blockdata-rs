# Internal Transfer Architecture - Final Design

**Version**: 2.1
**Date**: 2025-12-14
**Status**: ✅ Confirmed - Production Ready

---

## 1. Source of Truth

### Critical Understanding

| Service | Source of Truth | TigerBeetle Role |
|---------|-----------------|------------------|
| **Funding** | TigerBeetle | **Primary** - Direct operations |
| **Trading** | UBSCore (RAM + WAL) | **Shadow** - Async replay for query/verify |

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        SOURCE OF TRUTH ARCHITECTURE                         │
├─────────────────────────────────────┬───────────────────────────────────────┤
│           FUNDING                   │             TRADING                   │
│                                     │                                       │
│   ┌─────────────────────────┐       │   ┌─────────────────────────┐         │
│   │      TigerBeetle        │       │   │        UBSCore          │         │
│   │   (SOURCE OF TRUTH)     │       │   │   (SOURCE OF TRUTH)     │         │
│   │                         │       │   │                         │         │
│   │   • User Funding Accts  │       │   │   • In-Memory State     │         │
│   │   • Freeze/Commit/Void  │       │   │   • WAL for Durability  │         │
│   │   • Direct API          │       │   │   • Aeron IPC Interface │         │
│   └─────────────────────────┘       │   └────────────┬────────────┘         │
│                                     │                │                      │
│                                     │                │ async replay         │
│                                     │                ▼                      │
│                                     │   ┌─────────────────────────┐         │
│                                     │   │      TigerBeetle        │         │
│                                     │   │   (SHADOW ONLY)         │         │
│                                     │   │                         │         │
│                                     │   │   • Query balances      │         │
│                                     │   │   • Verify correctness  │         │
│                                     │   │   • NOT authoritative   │         │
│                                     │   └─────────────────────────┘         │
└─────────────────────────────────────┴───────────────────────────────────────┘
```

---

## 2. Why FSM 2-Phase Is Required

### Cannot Use TigerBeetle Atomic Transfers

**Reason**: Funding and Trading are in **DIFFERENT systems**

```
Funding Account ──── TigerBeetle (authoritative)
    │
    │  DIFFERENT SYSTEMS
    │  Cannot use TB atomic/linked transfers!
    │
Trading Account ──── UBSCore RAM (authoritative)
                         │
                         └──→ TigerBeetle (shadow, async)
```

**TB shadow for Trading**:
- ❌ Async replay of UBSCore WAL
- ❌ Not real-time
- ❌ Cannot be used for authoritative operations
- ✅ Only for balance queries and verification

---

## 3. FSM 2-Phase Commit Flow

### State Machine

```
                              SourceFail
    ┌──────────────────────────────────────────────────────┐
    │                                                      │
    │  ┌──────┐  SourceCall   ┌───────────────┐            ▼
    │  │ Init │──────────────►│ SourcePending │        ┌────────┐
    │  └──┬───┘               └───────┬───────┘        │ Failed │
    │     │                           │                └────────┘
    │     │ SourceOk                  │ SourceOk
    │     │                           │
    │     ▼                           ▼
    │  ┌─────────────────────────────────┐
    └──│          SourceDone             │
       └───────────────┬─────────────────┘
                       │
          ┌────────────┼────────────┐
          │ TargetCall │ TargetOk   │ TargetFail
          ▼            │            ▼
   ┌──────────────┐    │    ┌──────────────┐
   │TargetPending │    │    │ Compensating │──────► RolledBack
   └──────┬───────┘    │    └──────────────┘
          │            │
   TargetOk            │
          │            │
          ▼            ▼
   ┌─────────────────────┐
   │     Committed ✅     │
   └─────────────────────┘
```

---

## 4. Transfer Flows

### 4.1 Funding → Trading

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                       FUNDING → TRADING TRANSFER                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Phase 1: Source Withdraw (Funding via TigerBeetle)                         │
│  ──────────────────────────────────────────────────                         │
│      TbFundingAdapter.withdraw()                                            │
│          │                                                                  │
│          ▼                                                                  │
│      TigerBeetle: Create PENDING transfer                                   │
│          • Debit user's Funding account                                     │
│          • Funds are FROZEN (pending_debits)                                │
│          • User cannot use these funds                                      │
│                                                                             │
│  Phase 2: Target Deposit (Trading via UBSCore)                              │
│  ─────────────────────────────────────────────                              │
│      TbTradingAdapter.deposit() or UbsTradingAdapter.deposit()              │
│          │                                                                  │
│          ▼                                                                  │
│      UBSCore (via Aeron IPC):                                               │
│          • Credit user's Trading balance in RAM                             │
│          • Write to WAL for durability                                      │
│          • Async: WAL replays to TigerBeetle shadow                         │
│                                                                             │
│  Phase 3: Finalize                                                          │
│  ────────────────                                                           │
│      TbFundingAdapter.commit()                                              │
│          │                                                                  │
│          ▼                                                                  │
│      TigerBeetle: POST_PENDING_TRANSFER                                     │
│          • Release frozen funds                                             │
│          • Transfer is now permanent                                        │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 Trading → Funding

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                       TRADING → FUNDING TRANSFER                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Phase 1: Source Withdraw (Trading via UBSCore)                             │
│  ──────────────────────────────────────────────                             │
│      TbTradingAdapter.withdraw() or UbsTradingAdapter.withdraw()            │
│          │                                                                  │
│          ▼                                                                  │
│      UBSCore (via Aeron IPC):                                               │
│          • Debit user's Trading balance in RAM                              │
│          • Write to WAL                                                     │
│          • Funds removed immediately (no freeze, direct debit)              │
│                                                                             │
│  Phase 2: Target Deposit (Funding via TigerBeetle)                          │
│  ─────────────────────────────────────────────────                          │
│      TbFundingAdapter.deposit()                                             │
│          │                                                                  │
│          ▼                                                                  │
│      TigerBeetle: Create direct transfer                                    │
│          • Credit user's Funding account                                    │
│          • Transfer is immediate                                            │
│                                                                             │
│  Phase 3: Finalize                                                          │
│  ────────────────                                                           │
│      TbTradingAdapter.commit() → No-op (already complete)                   │
│                                                                             │
│  Compensation (if Phase 2 fails)                                            │
│  ───────────────────────────────                                            │
│      TbTradingAdapter.rollback()                                            │
│          │                                                                  │
│          ▼                                                                  │
│      UBSCore (via Aeron IPC):                                               │
│          • Credit back user's Trading balance                               │
│          • Reverse the debit                                                │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 5. Service Adapter Interface

```rust
#[async_trait]
pub trait ServiceAdapter: Send + Sync {
    /// Withdraw/freeze funds from source
    /// - Funding: Create PENDING transfer in TigerBeetle
    /// - Trading: Debit balance in UBSCore RAM
    async fn withdraw(&self, req_id, user_id, asset_id, amount) -> OpResult;

    /// Deposit funds to target
    /// - Funding: Direct credit in TigerBeetle
    /// - Trading: Credit balance in UBSCore RAM
    async fn deposit(&self, req_id, user_id, asset_id, amount) -> OpResult;

    /// Commit a pending withdraw
    /// - Funding: POST_PENDING_TRANSFER in TigerBeetle
    /// - Trading: No-op (already complete)
    async fn commit(&self, req_id) -> OpResult;

    /// Rollback a failed transfer
    /// - Funding: VOID_PENDING_TRANSFER in TigerBeetle
    /// - Trading: Reverse credit in UBSCore
    async fn rollback(&self, req_id) -> OpResult;

    /// Query operation status
    async fn query(&self, req_id) -> OpResult;

    /// Adapter name for logging
    fn name(&self) -> &str;
}
```

---

## 6. Adapter Implementations

### 6.1 FundingAdapter (TigerBeetle-backed)

| Method | TigerBeetle Operation | Effect |
|--------|----------------------|--------|
| `withdraw` | Create PENDING transfer | `debits_pending += amount` |
| `deposit` | Create direct transfer | `credits_posted += amount` |
| `commit` | POST_PENDING_TRANSFER | `debits_posted += amount`, `debits_pending -= amount` |
| `rollback` | VOID_PENDING_TRANSFER | `debits_pending -= amount` |

### 6.2 TradingAdapter (UBSCore-backed)

| Method | UBSCore Operation | Effect |
|--------|------------------|--------|
| `withdraw` | Aeron IPC → Debit | RAM balance decreases |
| `deposit` | Aeron IPC → Credit | RAM balance increases |
| `commit` | No-op | Already complete |
| `rollback` | Aeron IPC → Reverse | RAM balance restored |

---

## 7. Safety Guarantees

### What's Guaranteed

| Scenario | Outcome |
|----------|---------|
| Happy path | SourceOk → TargetOk → **Committed** |
| Source fails | SourceFail → **Failed** (no changes) |
| Target fails | SourceOk → TargetFail → **Compensating** → RolledBack |
| Crash before source commit | On restart: retry from current state |
| Crash after source, before target | On restart: retry target deposit |
| Crash during compensation | On restart: retry rollback |

### Worst Case

```
Source FROZEN (Funding pending) + Target NOT credited yet + System crash
    │
    ▼
Recovery: Worker retries from SourceDone state
    │
    ▼
Either: Target succeeds → Committed
    Or: Target fails → Compensation → RolledBack (source unfrozen)
```

**No funds can be permanently lost** - FSM ensures eventual consistency.

---

## 8. Architecture Summary

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           INTERNAL TRANSFER V2                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                        TRANSFER COORDINATOR                          │   │
│   │                                                                      │   │
│   │   • FSM-based state management                                       │   │
│   │   • Uses ServiceAdapter abstraction ONLY                             │   │
│   │   • Does NOT know implementation details                             │   │
│   │   • Handles 2-phase commit + compensation                            │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│                    ┌───────────────┴───────────────┐                        │
│                    ▼                               ▼                        │
│   ┌────────────────────────────┐   ┌────────────────────────────┐          │
│   │     FundingAdapter         │   │     TradingAdapter          │          │
│   │                            │   │                             │          │
│   │   Implements:              │   │   Implements:               │          │
│   │   ServiceAdapter trait     │   │   ServiceAdapter trait      │          │
│   │                            │   │                             │          │
│   │   Backend:                 │   │   Backend:                  │          │
│   │   TigerBeetle (direct)     │   │   UBSCore (Aeron IPC)       │          │
│   │                            │   │                             │          │
│   │   Operations:              │   │   Operations:               │          │
│   │   • Pending transfers      │   │   • RAM balance updates     │          │
│   │   • Post/Void pending      │   │   • WAL durability          │          │
│   └────────────────────────────┘   └────────────────────────────┘          │
│                │                               │                            │
│                ▼                               ▼                            │
│   ┌────────────────────────────┐   ┌────────────────────────────┐          │
│   │       TigerBeetle          │   │         UBSCore            │          │
│   │   (SOURCE OF TRUTH)        │   │   (SOURCE OF TRUTH)        │          │
│   │                            │   │                             │          │
│   │   Funding Account:         │   │   Trading Account:          │          │
│   │   user_id + asset_id       │   │   user_id + asset_id        │          │
│   └────────────────────────────┘   └──────────────┬─────────────┘          │
│                                                    │                        │
│                                                    │ async WAL replay       │
│                                                    ▼                        │
│                                    ┌────────────────────────────┐          │
│                                    │       TigerBeetle          │          │
│                                    │   (SHADOW - query only)    │          │
│                                    └────────────────────────────┘          │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 9. Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| FSM 2-phase commit | Different sources of truth require cross-system coordination |
| ServiceAdapter abstraction | Coordinator decoupled from backend implementations |
| Compensation on failure | Ensures no partial state when target fails |
| TigerBeetle pending for Funding | Provides atomic freeze/commit/void |
| UBSCore direct for Trading | RAM is source of truth, TB is only shadow |

---

**Document Created**: 2025-12-14
**Author**: System Architecture Team
**Status**: Approved for Production
