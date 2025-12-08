# UBSCore: High-Frequency Exchange Architecture

> **The Complete Design & Implementation Philosophy for a World-Class Exchange**

## What is This?

This documentation describes the architecture for a **Validating Sequencer** - the core pattern used by the world's most robust exchanges (LMAX, Coinbase, Kraken).

**UBSCore** (User Balance Service Core) is the central authority that:
1. **Validates** orders against real-time balance state
2. **Sequences** them to a durable Write-Ahead Log
3. **Forwards** only valid orders to matching engines

## Quick Start

| If you want to... | Read this |
|-------------------|-----------|
| Understand the overall architecture | [UBSCORE_SUMMARY.md](./UBSCORE_SUMMARY.md) |
| Implement the balance service | [UBSCORE_ARCHITECTURE.md](./UBSCORE_ARCHITECTURE.md) |
| Build crash-safe persistence | [WAL_SAFETY.md](./WAL_SAFETY.md) |
| Handle edge cases safely | [GHOST_MONEY_HANDLING.md](./GHOST_MONEY_HANDLING.md) |

## The Golden Rule

> **"UBSCore never asks. It knows."**

- Never queries a database during trading
- Never calls an external API
- If data isn't in RAM, it doesn't exist

## 🚨 Critical: Internal vs Client Naming

| `Client*` (Gateway) | `Internal*` (UBSCore) |
|---------------------|------------------------|
| Decimals, Strings | Raw u64 |
| `ClientOrder` | `InternalOrder` |

**See**: [UBSCORE_ARCHITECTURE.md](./UBSCORE_ARCHITECTURE.md#-critical-internal-vs-client-struct-naming-)

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CLIENT                                          │
│                         (JSON, WebSocket)                                    │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GATEWAY                                         │
│   Auth • Rate Limit • JSON→Binary • Route by user_id                        │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Aeron UDP (~10-50 µs)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                          UBSCore (THE BRAIN)                                 │
│                                                                              │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  In-Memory State: HashMap<UserId, Account>                          │   │
│   │  WAL: O_DIRECT + fsync, Group Commit, CRC32 Framing                │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│   "Validate in RAM → Persist to WAL → Forward to ME"                        │
└─────────────────────────────────────────────────────────────────────────────┘
                │                                         │
                │ Aeron IPC (~100 ns)                     │ Async
                ▼                                         ▼
┌───────────────────────────────┐       ┌─────────────────────────────────────┐
│    MATCHING ENGINE            │       │        OFFSHORE (Cold)              │
│    (NO balance checks!)       │       │   Redpanda • S3 • ScyllaDB          │
└───────────────────────────────┘       └─────────────────────────────────────┘
```

## Performance Targets

| Metric | Target | How |
|--------|--------|-----|
| Order Entry Latency | ~50 µs | RAM validation + batched WAL |
| Balance Check | ~100 ns | In-memory HashMap |
| WAL Write (batched) | ~10-20 µs | O_DIRECT + Group Commit |
| ME Communication | ~100 ns | Aeron IPC (shared memory) |
| Capital Recycling | <1 µs | Hot Path speculative credits |

## Documentation Index

### Core Architecture

| Document | Description |
|----------|-------------|
| [**UBSCORE_SUMMARY.md**](./UBSCORE_SUMMARY.md) | Executive summary, key decisions, latency budget |
| [**UBSCORE_ARCHITECTURE.md**](./UBSCORE_ARCHITECTURE.md) | Full implementation: multi-core, Spot/Futures isolation |
| [**RISK_FIRST_ARCHITECTURE.md**](./RISK_FIRST_ARCHITECTURE.md) | Pre-trade risk firewall, balance locking |

### Persistence & Safety

| Document | Description |
|----------|-------------|
| [**WAL_SAFETY.md**](./WAL_SAFETY.md) | O_DIRECT, CRC32, group commit, rotation, dual disk |
| [**GHOST_MONEY_HANDLING.md**](./GHOST_MONEY_HANDLING.md) | Debt model, auto-liquidation, withdrawal firewall |

### Communication

| Document | Description |
|----------|-------------|
| [**TRANSPORT_AERON_VS_ZEROMQ.md**](./TRANSPORT_AERON_VS_ZEROMQ.md) | Why Aeron: IPC, UDP, no head-of-line blocking |
| [**GATEWAY_INGRESS.md**](./GATEWAY_INGRESS.md) | Order entry: client sharding, protocol normalization |
| [**SBE_SERIALIZATION.md**](./SBE_SERIALIZATION.md) | Zero-copy: SBE, rkyv, Bincode comparison |

### Advanced Patterns

| Document | Description |
|----------|-------------|
| [**SPECULATIVE_EXECUTION.md**](./SPECULATIVE_EXECUTION.md) | Hot/Cold path for sub-µs capital recycling |
| [**FAN_IN_SERIALIZATION.md**](./FAN_IN_SERIALIZATION.md) | Kafka as serializer for deterministic replay |

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Topology** | Gateway → UBSCore → ME | ME never wastes CPU on invalid orders |
| **Persistence** | Synchronous WAL | Ghost Money is impossible |
| **Transport** | Aeron IPC/UDP | No TCP head-of-line blocking |
| **Storage** | O_DIRECT + fdatasync | No OS flush latency spikes |
| **Serialization** | rkyv (Rust) or SBE | Zero-copy, ~15ns decode |
| **Rotation** | Pre-allocated swap | No file creation in hot path |
| **Snapshots** | Replica service | Not fork() in multi-threaded Rust |
| **Multi-Core** | Spot/Futures isolated | Futures math doesn't lag Spot |

## The Pattern: Validating Sequencer

```
1. VALIDATE:  Check balance in RAM              (~100 ns)
2. SEQUENCE:  Persist to WAL                    (~10-20 µs batched)
3. EXECUTE:   Forward to Matching Engine        (~100 ns)

Total: ~50 µs end-to-end
```

This is the architecture used by **LMAX, Coinbase, Kraken** - the world's most robust exchanges.

## Hardware Requirements

| Component | Requirement | Why |
|-----------|-------------|-----|
| **NVMe** | Enterprise with PLP | Power Loss Protection capacitors |
| **RAM** | ECC, sufficient for all balances | ~100 bytes per user |
| **Network** | Low-latency NIC (Solarflare/Mellanox) | Kernel bypass for UDP |
| **CPU** | High single-thread performance | Pin critical threads |

## Code Examples

| File | Description |
|------|-------------|
| [examples/aeron_hot_path.rs](../examples/aeron_hot_path.rs) | Aeron IPC implementation for ~100ns messaging |

## Summary

UBSCore is not a microservice. It's a **state machine** that:

- Holds the **authoritative balance state** in RAM
- Enforces **pre-trade risk** before orders reach matching
- Guarantees **crash safety** via synchronous WAL
- Enables **sub-microsecond capital recycling** via speculative execution

Build this, and you have the foundation of a world-class exchange.
