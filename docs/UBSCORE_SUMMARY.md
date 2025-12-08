# UBSCore: Executive Summary

## What is UBSCore?

**UBSCore** (User Balance Service Core) is the central authority of the exchange. It unifies:
- **Pre-Trade Risk**: Validates orders before matching
- **Balance Management**: In-memory double-entry bookkeeping
- **Persistence**: Write-Ahead Log for durability

## Architectural Role

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              TOPOLOGY                                        │
│                                                                              │
│   Gateway ──────────▶ UBSCore ──────────▶ Matching Engine                   │
│                       (Brain)              (Muscle)                          │
│                                                                              │
│   Philosophy: "Inshore" - Lock funds in RAM BEFORE order reaches ME         │
│   Source of Truth: The WAL on UBSCore's disk                                │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## The Golden Rule

> **"UBSCore never asks. It knows."**

- Never queries a database
- Never calls an external API
- If data isn't in `self.ram`, the data doesn't exist

## 🚨 Critical Naming Convention

| Layer | Prefix | Values | Example |
|-------|--------|--------|---------|
| **Gateway/API** | `Client*` | Decimals | `ClientOrder { price: "50000.00" }` |
| **UBSCore** | `Internal*` | Raw u64 | `InternalOrder { price: 5000000000000 }` |

**Conversion**: Gateway is the ONLY place `Client* → Internal*` happens.

**See**: [UBSCORE_ARCHITECTURE.md](./UBSCORE_ARCHITECTURE.md) for complete rules.

## Implementation Details

### A. State Management (The "Hot" Zone)

| Component | Implementation |
|-----------|----------------|
| **Data Structure** | `HashMap<UserId, Account>` sharded by `user_id` |
| **Bookkeeping** | Double-entry: `Available`, `Frozen`, `Speculative` |
| **Idempotency** | In-memory Bitset Window (reject duplicate order IDs) |

**Multi-Core Differentiation**:

| Core | Computation | Isolation Reason |
|------|-------------|------------------|
| `UBSCore_Spot` | Simple arithmetic | Very fast |
| `UBSCore_Futures` | Margins, Greeks, PnL | Heavy CPU, isolated from Spot |

### B. Communication & Serialization

| Layer | Choice | Rationale |
|-------|--------|-----------|
| **Transport** | Aeron (IPC/UDP) | Deterministic µs latency, shared memory |
| **Serialization** | rkyv or SBE | Zero parsing overhead (pointer cast) |
| **Alternative** | Bincode | Prototype-friendly, swap later |

### C. Persistence (The "Safe" Zone)

**Workflow**: "Check → Publish → Poll"

```
1. Check:   Validate balance in RAM
2. Publish: Send valid order to WAL (Aeron Archive or O_DIRECT)
3. Poll:    Wait for disk ACK (recordingPosition or fsync)
4. Update:  Mutate RAM state
5. ACK:     Notify user
```

| Component | Requirement |
|-----------|-------------|
| **Hardware** | Enterprise NVMe with PLP (Power Loss Protection) |
| **IO Method** | O_DIRECT + fdatasync |
| **Batching** | Group Commit (10-50 events per fsync) |
| **Format** | `[Length][CRC32][Payload]` |

### D. Lifecycle & Recovery

| Phase | Strategy |
|-------|----------|
| **Snapshots** | Triggered via Replica Sidecar (avoids blocking main loop) |
| **WAL Pruning** | Janitor deletes after S3 upload + Snapshot confirmation |
| **Backups** | S3 Sidecar (Multipart) or ZFS Send for active redundancy |
| **Recovery** | Replay WAL from last snapshot, stop at first CRC error |

## Critical Decision Choices

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Routing Topology** | Gateway → UBSCore → ME | ME never wastes CPU on "insufficient funds" orders |
| **Persistence Model** | Synchronous WAL | "Fire and Forget" allows Ghost Money; sync guarantees solvency |
| **Network Protocol** | Aeron | ZeroMQ (TCP) has head-of-line blocking; Aeron is jitter-free |
| **Storage IO** | O_DIRECT | Standard `write()` causes 50ms+ spikes during OS background flush |
| **Log Rotation** | Pre-Allocated Swap | Creating files takes ms; swap pre-created files instantly |
| **Snapshotting** | Replica Service | `fork()` is risky in multi-threaded Rust; dedicated replica is safe |

## The Processing Loop

```rust
pub struct UBSCore {
    accounts: HashMap<UserId, Account>,
    wal: GroupCommitWal,
    me_publisher: AeronPublication,
}

impl UBSCore {
    pub fn process_order(&mut self, order: Order) -> Result<(), RejectReason> {
        // 1. Validate in RAM (instant)
        let account = self.accounts.get_mut(&order.user_id)
            .ok_or(RejectReason::AccountNotFound)?;

        if account.available < order.cost {
            return Err(RejectReason::InsufficientBalance);
        }

        // 2. Pre-lock funds (RAM mutation)
        account.available -= order.cost;
        account.frozen += order.cost;

        // 3. Persist to WAL (batched, will fsync on batch boundary)
        self.wal.append(&order.to_bytes());

        // 4. Forward to ME (Aeron IPC)
        self.me_publisher.offer(&order.to_sbe_bytes());

        Ok(())
    }

    pub fn on_trade(&mut self, trade: Trade) {
        // Hot Path: Immediate speculative credit
        let buyer = self.accounts.get_mut(&trade.buyer_id).unwrap();
        buyer.speculative += trade.base_qty;

        // Cold Path: WAL + downstream persistence (async)
        self.wal.append(&trade.to_bytes());
    }
}
```

## Latency Budget

| Stage | Latency | Notes |
|-------|---------|-------|
| Gateway → UBSCore | 10-50 µs | Aeron UDP |
| Balance Check | ~100 ns | RAM lookup |
| WAL Append | ~200 ns | Buffer only |
| WAL Sync (batched) | 10-20 µs | fsync amortized over 50 orders |
| UBSCore → ME | ~100 ns | Aeron IPC (shared memory) |
| **Total Order Entry** | **~50 µs** | End-to-end |

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CLIENT                                          │
│                         (JSON, WebSocket)                                    │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GATEWAY                                         │
│   - Auth, Rate Limit, DDoS Protection                                       │
│   - JSON → Binary (rkyv/SBE)                                                │
│   - Route by user_id % shard_count                                          │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Aeron UDP (~10-50 µs)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                          UBSCore (THE BRAIN)                                 │
│                                                                              │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  HashMap<UserId, Account>                                           │   │
│   │  ┌─────────────────────────────────────────────────────────────┐   │   │
│   │  │ Account { available, frozen, speculative, version }         │   │   │
│   │  └─────────────────────────────────────────────────────────────┘   │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  WAL (O_DIRECT + fsync, Group Commit)                              │   │
│   │  Format: [Len][CRC32][Payload]                                     │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
          │                                           │
          │ Aeron IPC (~100 ns)                       │ Async Shipper
          ▼                                           ▼
┌─────────────────────────────┐         ┌─────────────────────────────────────┐
│    MATCHING ENGINE          │         │        OFFSHORE (Cold)              │
│    (NO balance checks!)     │         │   - Redpanda (persistence)          │
│                             │         │   - S3 (backups)                    │
│    Sharded by symbol_id     │         │   - ScyllaDB (queries)              │
└─────────────────────────────┘         └─────────────────────────────────────┘
```

## What UBSCore Is NOT

| Anti-Pattern | Why |
|--------------|-----|
| A database | It's RAM-first, WAL for durability |
| A REST service | It's a message processor |
| Queryable | No "get all users", only shard lookups |
| Stateless | It IS the state |

## Final Verdict

**You are building a Validating Sequencer.**

1. **Validate** the intent (balance check in RAM)
2. **Sequence** to disk (WAL persistence)
3. **Execute** (forward to Matching Engine)

This is the architecture used by **LMAX, Coinbase, Kraken** - the world's most robust exchanges.

---

## Related Documentation

| Document | Topic |
|----------|-------|
| `UBSCORE_ARCHITECTURE.md` | Full architecture with multi-core |
| `WAL_SAFETY.md` | WAL persistence, O_DIRECT, rotation |
| `SPECULATIVE_EXECUTION.md` | Hot/Cold path for capital recycling |
| `GHOST_MONEY_HANDLING.md` | Failure handling, auto-liquidation |
| `TRANSPORT_AERON_VS_ZEROMQ.md` | Why Aeron |
| `SBE_SERIALIZATION.md` | Zero-copy serialization |
| `GATEWAY_INGRESS.md` | Order entry design |
