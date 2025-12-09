# Trading System Architecture - Message Queue Based

## Overview

Pure message-queue architecture with strict service boundaries and event sourcing.

## Core Principles

1. **No Direct DB Writes**: Services NEVER write to other services' databases
2. **Message Queues Only**: All communication via Kafka/Redpanda
3. **Single Source of Truth**: Each service owns its domain
4. **Event Sourcing**: All operations generate events
5. **Replay-able**: Kafka retention allows replay/debugging

## Service Boundaries

### UBSCore (Balance Authority)
- **Owns**: User balance state (RAM + WAL)
- **Consumes**: `balance.operations` (deposits/withdrawals from Gateway)
- **Produces**: `balance.events` (for Settlement persistence)
- **DB**: NONE (pure in-memory with WAL)

### Matching Engine (Order Matching)
- **Owns**: Order books, matching logic
- **Consumes**: `orders` topic (validated orders from UBSCore)
- **Produces**: ZMQ (trades, balance events to Settlement)
- **DB**: NONE (pure in-memory with WAL)

### Settlement Service (Persistence Layer)
- **Owns**: ScyllaDB writes (all tables)
- **Consumes**:
  - ZMQ from ME (trades, order updates)
  - Kafka `balance.events` (deposits/withdrawals from UBSCore)
- **Produces**: NOTHING (end of pipeline)
- **DB**: ScyllaDB (owns all write operations)

### Gateway (HTTP→Kafka Bridge)
- **Owns**: HTTP API, request validation
- **Consumes**: HTTP requests
- **Produces**:
  - `balance.operations` (deposits/withdrawals)
  - `orders` (order requests to UBSCore)
- **DB**: READ-ONLY ScyllaDB (for API queries)

## Data Flows

### Deposit Flow
```
1. User → Gateway HTTP POST /transfer_in
2. Gateway → Kafka balance.operations (BalanceRequest::TransferIn)
3. UBSCore consumes, validates, updates RAM, writes WAL
4. UBSCore → Kafka balance.events (BalanceEvent)
5. Settlement consumes, writes to ScyllaDB balance_ledger table
6. Done! (Balance visible in DB)
```

### Order Flow
```
1. User → Gateway HTTP POST /orders
2. Gateway → UBSCore Aeron (order validation)
3. UBSCore validates balance, locks funds, writes WAL
4. UBSCore → Kafka orders (InternalOrder)
5. ME consumes, matches, generates trades
6. ME → ZMQ (EngineOutput with trades + balance events)
7. Settlement consumes, writes to ScyllaDB
8. Done! (Trades and balances in DB)
```

### Withdrawal Flow
```
1. User → Gateway HTTP POST /transfer_out
2. Gateway → Kafka balance.operations (BalanceRequest::TransferOut)
3. UBSCore validates, unlocks/debits RAM, writes WAL
4. UBSCore → Kafka balance.events (BalanceEvent)
5. Settlement writes to ScyllaDB
6. Done!
```

## Kafka Topics

| Topic | Producer | Consumer | Message Type | Purpose |
|-------|----------|----------|--------------|---------|
| `balance.operations` | Gateway | UBSCore | `BalanceRequest` | Deposit/withdrawal requests |
| `balance.events` | UBSCore | Settlement | `BalanceEvent` | Balance changes to persist |
| `orders` | UBSCore | ME | `InternalOrder` | Validated orders for matching |

## ZMQ Sockets

| Socket | Publisher | Subscriber | Message Type | Purpose |
|--------|-----------|------------|--------------|---------|
| tcp://localhost:5558 | ME | Settlement | `EngineOutput` | Trades + balance events |

## Database Ownership

### ScyllaDB Tables

**Owner: Settlement Service ONLY**

- `balance_ledger` - All balance events (immutable log)
- `user_balances` - Current balance snapshots (deprecated, use ledger)
- `settled_trades` - All trades
- `order_history` - Order lifecycle events
- `engine_outputs` - ME output sequence/hash chain
- `chain_state` - Recovery metadata

**READ-ONLY Access**: Gateway (for API queries)

### In-Memory State

- **UBSCore**: Balance state (authoritative, RAM + WAL)
- **ME**: Order books (authoritative, RAM + WAL)

## Message Queue Setup

### Redpanda (Kafka-compatible)

**Required Topics** (auto-create or pre-create):
```bash
rpk topic create balance.operations --partitions 4
rpk topic create balance.events --partitions 4
rpk topic create orders --partitions 4
```

**Configuration**:
- `auto_create_topics_enabled: true` (recommended for dev)
- `default_topic_partitions: 4`
- `log_retention_ms: 604800000` (7 days)

## Service Dependencies

```
Gateway
  ├── Consumes: HTTP (port 3001)
  ├── Produces: Kafka (balance.operations)
  └── Reads: ScyllaDB (balance queries)

UBSCore
  ├── Consumes: Kafka (balance.operations)
  │             Aeron UDP (order validation)
  ├── Produces: Kafka (balance.events, orders)
  │             Aeron UDP (order responses)
  └── Persists: WAL only (no DB)

Matching Engine
  ├── Consumes: Kafka (orders)
  ├── Produces: ZMQ (Engine Outputs)
  └── Persists: WAL only (no DB)

Settlement
  ├── Consumes: ZMQ (from ME)
  │             Kafka (balance.events from UBSCore)
  ├── Writes: ScyllaDB (ALL tables)
  └── Produces: NOTHING
```

## Benefits of This Architecture

### Decoupling
- Services communicate only via messages
- No direct database coupling
- Can deploy/restart services independently

### Observability
- All events in Kafka (searchable, replay-able)
- Each service logs its processing
- Can add consumers for monitoring without touching services

### Durability
- Triple protection: WAL + Kafka + ScyllaDB
- Can recover from any component failure
- Kafka retention allows replay from any point

### Scalability
- Add more Settlement consumers for higher write throughput
- Partition Kafka topics by user_id for parallelism
- Scale read-only Gateway instances independently

### Testing
- Can replay Kafka messages for testing
- Can add test consumers to verify events
- Services don't need DB for unit tests

## Anti-Patterns (DO NOT DO)

❌ **Service A writes to Service B's database directly**
  - Violates ownership boundaries
  - Creates hidden coupling
  - Makes recovery/debugging impossible

❌ **Synchronous RPC between services for critical path**
  - Use async messages instead
  - RPC for queries only (Gateway read-only)

❌ **Shared database between services**
  - Settlement owns all ScyllaDB writes
  - Others can READ-ONLY via Settlement's tables

## Migration Notes

### Phase 1 (DONE ✅)
- UBSCore balance operations implemented
- Gateway publishes to Kafka

### Phase 2 (DONE ✅)
- Removed ME GlobalLedger
- ME is pure matcher
- UBSCore publishes balance events
- Settlement consumes from Kafka

### Future Enhancements
- Add Prometheus metrics
- Add distributed tracing (OpenTelemetry)
- Add circuit breakers
- Add rate limiting

---

**Last Updated**: 2025-12-10
**Architecture Version**: 2.0 (Message Queue Based)
**Status**: Production Ready ✅
