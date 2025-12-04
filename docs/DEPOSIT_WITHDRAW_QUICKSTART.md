# Deposit/Withdrawal System - Quick Start Guide

## Overview

The deposit/withdrawal system consists of three main components:

1. **deposit_withdraw_gateway** - HTTP API for submitting deposit/withdrawal requests
2. **balance_processor** - Kafka consumer that processes balance operations
3. **balance_test_client** - Test client to demonstrate the flow

## Architecture

```
User/External System
        ↓
deposit_withdraw_gateway (HTTP API :8082)
        ↓
Redpanda (balance.operations topic)
        ↓
balance_processor (Kafka Consumer)
        ↓
GlobalLedger (Shared with matching_engine_server)
```

## Prerequisites

1. **Redpanda/Kafka** running on `localhost:9093`
2. **Config file** at `config/config.yaml` with `balance_ops` topic configured

## Running the System

### Step 1: Start Redpanda (if not already running)

```bash
docker run -d --name redpanda \
  -p 9092:9092 \
  -p 9093:9093 \
  docker.redpanda.com/redpandadata/redpanda:latest \
  redpanda start --smp 1 --memory 1G
```

### Step 2: Start Balance Processor

```bash
cargo run --bin balance_processor --release
```

**Expected Output:**
```
--------------------------------------------------
Balance Processor Started
  Kafka Broker:      localhost:9093
  Balance Topic:     balance.operations
  Consumer Group:    balance_processor_group
  WAL Directory:     "balance_processor_wal"
  Snapshot Dir:      "balance_processor_snapshots"
--------------------------------------------------
```

### Step 3: Start Deposit/Withdraw Gateway (in another terminal)

```bash
cargo run --bin deposit_withdraw_gateway --release
```

**Expected Output:**
```
--------------------------------------------------
Deposit/Withdraw Gateway Started
  Listening on:      0.0.0.0:8082
  Kafka Broker:      localhost:9093
  Balance Topic:     balance.operations
--------------------------------------------------
Endpoints:
  POST /api/v1/deposit
  POST /api/v1/withdraw
  GET  /health
--------------------------------------------------
```

### Step 4: Test the System (in another terminal)

```bash
cargo run --bin balance_test_client --release
```

## Manual Testing with curl

### Test 1: Deposit (Blockchain)

```bash
curl -X POST http://localhost:8082/api/v1/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "deposit_btc_001",
    "user_id": 1001,
    "asset_id": 1,
    "amount": 100000000,
    "chain": "Bitcoin",
    "external_tx_id": "0x1234567890abcdef",
    "confirmations": 6,
    "required_confirmations": 6
  }'
```

**Expected Response:**
```json
{
  "success": true,
  "message": "Deposit request submitted",
  "request_id": "deposit_btc_001"
}
```

**Balance Processor Output:**
```
📨 Received: Deposit { request_id: "deposit_btc_001", ... }
   ✓ Blockchain deposit validated: 6 confirmations on Bitcoin (required: 6)
✅ Deposit processed: 100000000 units of asset 1 for user 1001 (tx: 0x1234567890abcdef)
```

### Test 2: Deposit with Insufficient Confirmations

```bash
curl -X POST http://localhost:8082/api/v1/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "deposit_eth_002",
    "user_id": 1002,
    "asset_id": 3,
    "amount": 5000000000000000000,
    "chain": "Ethereum",
    "external_tx_id": "0xabcdef1234567890",
    "confirmations": 5,
    "required_confirmations": 12
  }'
```

**Balance Processor Output:**
```
📨 Received: Deposit { request_id: "deposit_eth_002", ... }
   ✗ Insufficient confirmations: 5 on Ethereum (required: 12)
❌ Deposit validation failed: deposit_eth_002
```

### Test 3: Withdrawal

```bash
curl -X POST http://localhost:8082/api/v1/withdraw \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "withdraw_btc_003",
    "user_id": 1001,
    "asset_id": 1,
    "amount": 50000000,
    "chain": "Bitcoin",
    "address": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"
  }'
```

**Balance Processor Output:**
```
📨 Received: Withdraw { request_id: "withdraw_btc_003", ... }
🔒 Withdrawal locked: 50000000 units of asset 1 for user 1001 to bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh
   Destination: Blockchain { chain: "Bitcoin", address: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh" }
   ⏳ Awaiting external confirmation...
```

### Test 4: Idempotency Test (Duplicate Deposit)

```bash
# Send the same deposit request again
curl -X POST http://localhost:8082/api/v1/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "deposit_btc_001",
    "user_id": 1001,
    "asset_id": 1,
    "amount": 100000000,
    "chain": "Bitcoin",
    "external_tx_id": "0x1234567890abcdef",
    "confirmations": 6,
    "required_confirmations": 6
  }'
```

**Balance Processor Output:**
```
📨 Received: Deposit { request_id: "deposit_btc_001", ... }
⚠️  Duplicate deposit request: deposit_btc_001
```

### Test 5: Health Check

```bash
curl http://localhost:8082/health
```

**Response:**
```json
{
  "success": true,
  "message": "Deposit/Withdraw Gateway is healthy",
  "request_id": null
}
```

## Checking Ledger State

The balance processor uses the same ledger as the matching engine. To verify balances, you can:

1. Check the ledger WAL files in `balance_processor_wal/`
2. Check the ledger snapshots in `balance_processor_snapshots/`
3. Integrate with the matching engine to query balances

## Message Format

### Deposit Request
```json
{
  "type": "Deposit",
  "data": {
    "request_id": "unique_id",
    "user_id": 1001,
    "asset_id": 1,
    "amount": 100000000,
    "source": {
      "type": "Blockchain",
      "data": {
        "chain": "Bitcoin",
        "required_confirmations": 6
      }
    },
    "external_tx_id": "0x...",
    "confirmations": 6
  }
}
```

### Withdraw Request
```json
{
  "type": "Withdraw",
  "data": {
    "request_id": "unique_id",
    "user_id": 1001,
    "asset_id": 1,
    "amount": 50000000,
    "destination": {
      "type": "Blockchain",
      "data": {
        "chain": "Bitcoin",
        "address": "bc1q..."
      }
    },
    "external_address": "bc1q..."
  }
}
```

## Monitoring

### Balance Processor Stats

The balance processor prints stats every 10 seconds:

```
📊 Stats: 5 requests processed, 5 unique requests tracked
```

### Kafka Topic

Monitor the `balance.operations` topic:

```bash
docker exec -it redpanda rpk topic consume balance.operations
```

## Troubleshooting

### Issue: Gateway can't connect to Kafka

**Solution:** Check that Redpanda is running and accessible on `localhost:9093`

```bash
docker ps | grep redpanda
```

### Issue: Balance processor not receiving messages

**Solution:** Check that the topic exists and gateway is publishing

```bash
docker exec -it redpanda rpk topic list
docker exec -it redpanda rpk topic consume balance.operations
```

### Issue: Deposits not being processed

**Solution:** Check balance processor logs for validation failures (confirmations, etc.)

## Next Steps

1. **Add Centrifugo Integration** - Publish balance updates to users via WebSocket
2. **Add Withdrawal Tracking** - Implement tracking for WithdrawConfirm/WithdrawReject
3. **Add Database** - Store processed requests for persistence across restarts
4. **Add Authentication** - Implement JWT/API key authentication for gateway
5. **Add 2FA** - Require 2FA for withdrawals
6. **Add Limits** - Implement daily/monthly withdrawal limits

## Architecture Benefits

✅ **Isolation** - Balance operations don't interfere with order matching
✅ **Scalability** - Can scale balance processor independently
✅ **Idempotency** - Duplicate requests are safely ignored
✅ **Audit Trail** - Complete history in ledger WAL
✅ **Recovery** - Can recover from crashes using ledger snapshots + WAL
✅ **Async Processing** - Non-blocking via Kafka
