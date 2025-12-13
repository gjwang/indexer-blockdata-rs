# Trading Exchange Backend

A high-performance trading exchange backend written in Rust, featuring ultra-low latency order processing, real-time balance management, and atomic internal transfers.

## Overview

This trading exchange platform is designed for:

- **Ultra-low latency** order processing (~100μs via Aeron IPC)
- **Real-time balance management** with TigerBeetle
- **Atomic internal transfers** between Funding and Trading accounts
- **High throughput** message processing via Kafka

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Gateway   │────▶│   UBSCore   │────▶│  Matching   │
│  (HTTP API) │     │  (Balance)  │     │   Engine    │
└─────────────┘     └─────────────┘     └─────────────┘
       │                   │                   │
       │                   ▼                   │
       │            ┌─────────────┐            │
       │            │ TigerBeetle │            │
       │            │  (Ledger)   │            │
       │            └─────────────┘            │
       │                                       │
       ▼                                       ▼
┌─────────────────────────────────────────────────────┐
│                    Kafka                            │
└─────────────────────────────────────────────────────┘
       │                                       │
       ▼                                       ▼
┌─────────────┐                        ┌─────────────┐
│  Settlement │                        │   ScyllaDB  │
│   Service   │                        │ (Persistence)│
└─────────────┘                        └─────────────┘
```

## Key Features

### Internal Transfers V2

FSM-based internal transfer system with TigerBeetle atomic guarantees:

- **Funding ↔ Trading** account transfers
- **Atomic commits** via TigerBeetle linked transfers
- **Idempotent operations** - safe to retry
- **Compensation logic** for failure recovery

### UBSCore (Unified Balance Service)

Central balance management with risk checks:

- **Pre-trade risk validation**
- **Real-time balance updates**
- **Aeron IPC** for ultra-low latency

### High-Performance Infrastructure

- **Aeron** - ~100ns IPC latency
- **TigerBeetle** - Hardware-accelerated double-entry accounting
- **Kafka** - Distributed message streaming
- **ScyllaDB** - Low-latency persistence

## Quick Start

```bash
# Start infrastructure
docker-compose up -d

# Run tests
./tests/09_transfer_integration.sh
./tests/10_full_exchange_e2e.sh
```

## Documentation Sections

- **Architecture** - System design and component interactions
- **Core Components** - UBSCore, Gateway, Settlement details
- **Internal Transfers** - Transfer API and FSM design
- **Infrastructure** - TigerBeetle, Aeron, Kafka setup
- **Operations** - Testing and deployment guides
