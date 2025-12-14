# Internal Transfer Fix Plan

**Date**: 2025-12-14
**Status**: 🔴 CRITICAL - Implementation Misaligned with Architecture
**Priority**: P0 - Must Fix Before Production

---

## Problem Summary

| Component | Current State | Required State |
|-----------|---------------|----------------|
| `TbTradingAdapter` | Uses TigerBeetle directly | Should use UBSCore via Aeron |
| Trading Balance | Written to TB | Should be written to UBSCore RAM |
| Source of Truth | TB (wrong) | UBSCore RAM + WAL (correct) |

---

## Architecture Reminder

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CORRECT: Trading uses UBSCore as Source of Truth                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│   TransferCoordinator                                                       │
│         │                                                                   │
│         ├── FundingAdapter ──────→ TigerBeetle (SOT) ✅                     │
│         │                                                                   │
│         └── TradingAdapter ──────→ UBSCore (Aeron IPC)                      │
│                                         │                                   │
│                                         ▼                                   │
│                                    RAM (SOT) + WAL                          │
│                                         │                                   │
│                                         ▼ (async)                           │
│                                    TigerBeetle (shadow)                     │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Fix Plan

### Phase 1: Create UbsTradingAdapter (2-3 hours)

**Goal**: Implement proper TradingAdapter that communicates with UBSCore via Kafka/Aeron

#### Step 1.1: Define Internal Transfer Message Types

```rust
// src/ubs_core/events.rs (or new file)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalTransferOp {
    Withdraw {
        req_id: u64,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    },
    Deposit {
        req_id: u64,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    },
    Rollback {
        req_id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalTransferResult {
    Success { req_id: u64 },
    Failed { req_id: u64, reason: String },
    InsufficientBalance { req_id: u64, available: u64, requested: u64 },
}
```

#### Step 1.2: Implement UbsTradingAdapter

```rust
// src/transfer/adapters/trading.rs

pub struct UbsTradingAdapter {
    /// Kafka producer to send requests to UBSCore
    producer: Arc<FutureProducer>,
    /// Kafka topic for internal transfer operations
    topic: String,
    /// Response channel (for async response handling)
    response_rx: Arc<Mutex<HashMap<u64, oneshot::Sender<InternalTransferResult>>>>,
}

impl ServiceAdapter for UbsTradingAdapter {
    async fn withdraw(&self, req_id, user_id, asset_id, amount) -> OpResult {
        // 1. Send InternalTransferOp::Withdraw to UBSCore via Kafka
        // 2. Wait for response (with timeout)
        // 3. Map response to OpResult
    }

    async fn deposit(&self, ...) -> OpResult { ... }
    async fn commit(&self, req_id) -> OpResult { OpResult::Success }  // No-op for Trading
    async fn rollback(&self, req_id) -> OpResult { ... }
}
```

#### Step 1.3: Modify UBSCore to Handle Internal Transfers

```rust
// src/bin/ubscore_[aeron|kafka]_service.rs

// Add handler for InternalTransferOp messages
match message {
    InternalTransferOp::Withdraw { req_id, user_id, asset_id, amount } => {
        // 1. Check balance in RAM
        // 2. If sufficient: debit balance, write to WAL
        // 3. Publish InternalTransferResult::Success
        // 4. Async: sync to TigerBeetle shadow
    }
    InternalTransferOp::Deposit { ... } => { ... }
    InternalTransferOp::Rollback { req_id } => { ... }
}
```

---

### Phase 2: Update internal_transfer_service (30 min)

**Goal**: Use UbsTradingAdapter for Trading operations

```rust
// src/bin/internal_transfer_service.rs

// Change from:
let trading = Arc::new(TbTradingAdapter::new(tb_client.clone()));

// To:
let trading = Arc::new(UbsTradingAdapter::new(
    kafka_producer.clone(),
    "internal.transfers".to_string(),
));
```

---

### Phase 3: Update Kafka Topics (15 min)

**Goal**: Add new topics for internal transfer communication

```yaml
# config/dev.yaml
kafka:
  topics:
    internal_transfer_ops: "internal.transfer.ops"     # To UBSCore
    internal_transfer_results: "internal.transfer.results"  # From UBSCore
```

---

### Phase 4: Integration Testing (1-2 hours)

**Goal**: Verify the full flow works correctly

```
Test 1: Funding → Trading
  1. TransferService calls FundingAdapter.withdraw() → TB pending
  2. TransferService calls UbsTradingAdapter.deposit() → Kafka → UBSCore
  3. UBSCore credits RAM, writes WAL, responds success
  4. TransferService calls FundingAdapter.commit() → TB post_pending
  5. UBSCore async syncs to TB shadow
  ✅ Verify: UBSCore RAM has correct balance

Test 2: Trading → Funding
  1. TransferService calls UbsTradingAdapter.withdraw() → Kafka → UBSCore
  2. UBSCore debits RAM, writes WAL, responds success
  3. TransferService calls FundingAdapter.deposit() → TB direct credit
  ✅ Verify: TB has correct balance, UBSCore RAM updated

Test 3: Failure & Compensation
  1. Source withdraw succeeds
  2. Target deposit fails
  3. Coordinator calls rollback
  ✅ Verify: Source balance restored
```

---

### Phase 5: Update Documentation (15 min)

Update `docs/INTERNAL_TRANSFER_ARCHITECTURE.md` with implementation details.

---

## Timeline

| Phase | Task | Time | Status |
|-------|------|------|--------|
| 1.1 | Define message types | 30 min | TODO |
| 1.2 | Implement UbsTradingAdapter | 1 hour | TODO |
| 1.3 | Modify UBSCore handlers | 1 hour | TODO |
| 2 | Update internal_transfer_service | 30 min | TODO |
| 3 | Update Kafka topics | 15 min | TODO |
| 4 | Integration testing | 2 hours | TODO |
| 5 | Update documentation | 15 min | TODO |
| **Total** | | **~5.5 hours** | |

---

## Files to Modify

| File | Change |
|------|--------|
| `src/ubs_core/events.rs` | Add InternalTransferOp, InternalTransferResult |
| `src/transfer/adapters/trading.rs` | Add UbsTradingAdapter |
| `src/bin/ubscore_aeron_service.rs` | Handle internal transfer ops |
| `src/bin/ubscore_kafka_service.rs` | Handle internal transfer ops |
| `src/bin/internal_transfer_service.rs` | Use UbsTradingAdapter |
| `config/dev.yaml` | Add new Kafka topics |
| `tests/09_transfer_integration.sh` | Update for new flow |

---

## Immediate Action

1. ⚠️ Mark current tests as **integration-only** (not production)
2. 🔴 Add warning to `TbTradingAdapter`: "FOR TESTING ONLY - Do not use in production"
3. 📋 Create tracking issue for this fix

---

## Questions to Resolve

1. **Kafka vs Aeron for internal transfers?**
   - Aeron: Lower latency, but requires embedded driver
   - Kafka: Higher latency (~1ms), but simpler setup

2. **Response handling**:
   - Sync wait with timeout?
   - Async callback?
   - Poll-based?

3. **UBSCore changes**:
   - Existing UBSCore service or separate handler?
   - Same Kafka consumer group?

---

**Reviewed by**: [Pending]
**Approved for implementation**: [YES/NO]
