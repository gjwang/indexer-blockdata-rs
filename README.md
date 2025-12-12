# Trading Exchange Backend

High-performance trading exchange backend in Rust, featuring:
- **UBSCore** - Ultra-low-latency matching engine with Aeron IPC
- **TigerBeetle** - Double-entry accounting for financial integrity
- **Internal Transfers** - FSM-based fund transfers between accounts

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Gateway Service                          │
│  (HTTP API - Orders, Deposits, Withdrawals, Internal Transfers) │
└─────────────────────────────────────────────────────────────────┘
                    │                    │
         Aeron IPC  │                    │  Kafka
                    ▼                    ▼
    ┌───────────────────────┐    ┌───────────────────────┐
    │     UBSCore Service   │    │  Internal Transfer    │
    │  (Matching + Balances)│    │      Service          │
    └───────────────────────┘    └───────────────────────┘
                    │                    │
                    ▼                    ▼
    ┌───────────────────────────────────────────────────────────┐
    │                      TigerBeetle                           │
    │             (Double-entry Accounting Ledger)               │
    └───────────────────────────────────────────────────────────┘
```

## Services

| Service | Description | Port |
|---------|-------------|------|
| `gateway_service` | HTTP API gateway | 8080 |
| `internal_transfer_service` | Internal transfer processing | 8080 (test mode) |
| `ubscore_aeron_service` | Matching engine with Aeron | N/A (IPC) |
| `matching_engine_server` | Kafka-based matching engine | N/A |
| `settlement_service` | Trade settlement | N/A |

## Quick Start

### Prerequisites

```bash
# Start infrastructure
docker-compose up -d

# Wait for ScyllaDB to be ready (30-60 seconds)
docker exec scylla cqlsh -e "SELECT now() FROM system.local"

# Apply schema
docker exec scylla cqlsh -e "$(cat schema/settlement_unified.cql)"
```

### Run Tests

```bash
# Core tests (standalone, requires ScyllaDB + TigerBeetle)
./tests/09_transfer_integration.sh    # Internal transfer tests
./tests/10_full_exchange_e2e.sh       # Full exchange E2E

# Infrastructure test
./tests/01_infrastructure.sh
```

### Development

```bash
# Build all binaries
cargo build

# Build with Aeron support (production)
cargo build --features aeron

# Run internal transfer service (test mode with HTTP)
cargo run --bin internal_transfer_service

# Run gateway service
cargo run --bin gateway_service
```

## API Endpoints

### Internal Transfers

```bash
# Create transfer (Funding → Trading)
curl -X POST http://localhost:8080/api/v1/transfer \
  -H "Content-Type: application/json" \
  -d '{"from":"funding","to":"trading","user_id":1001,"asset_id":1,"amount":1000}'

# Query transfer status
curl http://localhost:8080/api/v1/transfer/{req_id}
```

### Response Format

```json
{
  "req_id": "1851322281108712316",
  "status": "committed"
}
```

## Test Suite

| Test | Description | Requirements |
|------|-------------|--------------|
| `01_infrastructure.sh` | Infrastructure validation | Docker |
| `02_ubscore_startup.sh` | UBSCore startup | TigerBeetle + Aeron |
| `03_deposit_withdrawal.sh` | Deposit/withdrawal | Full stack |
| `04_http_api_test.sh` | HTTP API tests | Full stack |
| `05_gateway_e2e.sh` | Gateway E2E | Full stack |
| `06_matching_engine.sh` | Matching engine | Full stack |
| `07_settlement.sh` | Settlement | Full stack |
| `08_full_trading_e2e.sh` | Full trading | Full stack |
| `09_transfer_integration.sh` | Internal Transfer V2 | **ScyllaDB + TB** |
| `10_full_exchange_e2e.sh` | Full Exchange E2E | **ScyllaDB + TB** |

> Tests 09-10 are standalone and work with just ScyllaDB + TigerBeetle.

## Project Structure

```
src/
├── bin/
│   ├── gateway_service.rs           # Main API gateway
│   ├── internal_transfer_service.rs # Internal transfer processor
│   ├── ubscore_aeron_service.rs     # UBSCore with Aeron
│   ├── matching_engine_server.rs    # Kafka-based matching
│   └── settlement_service.rs        # Trade settlement
├── transfer/                        # Internal Transfer V2 module
│   ├── mod.rs                       # Module exports
│   ├── coordinator.rs               # FSM coordinator
│   ├── worker.rs                    # Background worker
│   ├── state.rs                     # FSM state machine
│   ├── adapters/                    # Service adapters
│   │   ├── funding.rs               # TigerBeetle funding
│   │   └── trading.rs               # TigerBeetle/UBSCore trading
│   ├── db.rs                        # ScyllaDB persistence
│   ├── queue.rs                     # Transfer queue
│   └── types.rs                     # Request types
├── ubs_core/                        # UBSCore engine
└── gateway.rs                       # Gateway routes
```

## Configuration

### Feature Flags

```toml
[features]
default = ["aeron"]  # Aeron enabled by default
aeron = ["rusteron-client", "rusteron-media-driver"]
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `TIGERBEETLE_ADDRESS` | TigerBeetle address | `3000` |
| `SCYLLA_ADDRESS` | ScyllaDB address | `127.0.0.1:9042` |
| `WORKER_ONLY` | Run as worker only (no HTTP) | `false` |
| `RUST_LOG` | Log level | `info` |

## License

Private - All rights reserved.
