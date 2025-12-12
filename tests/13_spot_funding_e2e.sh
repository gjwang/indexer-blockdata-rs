#!/bin/bash
# 13_spot_funding_e2e.sh
# E2E Test for Spot -> Funding internal transfers via UBSCore
#
# architecture:
# 1. API Request (Spot -> Funding)
# 2. Gateway -> Kafka (balance.operations)
# 3. UBSCore -> Consumes, Deduced Balance, Emits (balance.events)
# 4. Gateway (InternalTransferSettlement) -> Consumes events, Credits Funding (Mock/TB)

# Cleanup previous runs
pkill -f "target/debug/order_gate_server" || true
pkill -f "target/debug/ubscore_aeron_service" || true

set -e


echo "🚀 Starting Spot -> Funding E2E Test"

# 1. Start Services
# We need: Scylla, Kafka, TigerBeetle, OrderGateway, UBSCore (Aeron), Settlement (optional but good for logs)
# Use the standard start script but we might need to be specific to ensure clean state
# For simplicity, we assume robust start in 03_start_services.sh or we run them here.

# Kill any existing
pkill -f "order_gate_server" || true
pkill -f "ubscore_aeron_service" || true

# Check Kafka/Scylla/TB are up (manual check usually, or assume running dev env)
# We can use docker-compose check
docker compose ps

 # Ensure topics exist to avoid consumer metadata lag
 echo "🛠 Creating Kafka topics..."
 docker exec redpanda rpk topic create balance.operations balance.events -p 1 || true


# Build binaries
cargo build --bin order_gate_server
cargo build --bin ubscore_aeron_service

# Start UBSCore Service
echo "Starting UBSCore Service..."
RUST_LOG=info ./target/debug/ubscore_aeron_service > ubscore.log 2>&1 &
UBS_PID=$!
echo "UBSCore PID: $UBS_PID"

echo "⏳ Waiting for UBSCore to initialize..."
sleep 5

# Start Order Gateway
echo "Starting Order Gateway..."
RUST_LOG=info ./target/debug/order_gate_server > gateway.log 2>&1 &
GATEWAY_PID=$!
echo "Gateway PID: $GATEWAY_PID"

sleep 5

# 2. Setup Test Data
# We need a user with Spot Balance.
# Since UBSCore starts empty (in-memory usually, unless wal), we need to DEPOSIT first.
# Or we use matching engine to process a deposit?
# UBSCore handles deposits via `BalanceRequest::TransferIn`.
# We can trigger a `TransferIn` via API first to fund the Spot Account.

USER_ID=999
ASSET="BTC"
AMOUNT="1.5"

echo "💰 Funding Spot Account via Transfer-In (Funding -> Spot)..."
# In current impl, TransferIn is Funding->Spot.
# It requires locking Funding implementation.
# API: POST /api/v1/user/transfer_in
# Payload: { request_id, user_id, asset, amount }
# NOTE: This endpoint (transfer_in) implementation in gateway.rs (Step 1723)
# locks funding account and sends to UBSCore?
# Wait, let's check gateway.rs transfer_in logic.
# It calls state.funding_account.lock() then... sends to Kafka?
# If it sends to Kafka `balance.operations`, then UBSCore will process it and credit Spot.
# YES.

curl -X POST http://localhost:3001/api/v1/user/transfer_in \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "setup_001",
    "user_id": '$USER_ID',
    "asset": "'$ASSET'",
    "amount": "'$AMOUNT'"
  }'

sleep 2

# Verify Spot Balance (via Gateway API)
# GET /api/v1/user/balance?user_id=...
# This might read from TB or Scylla.
# UBSCore balance is in-memory/WAL.
# Does Gateway `get_balance` query UBSCore?
# Current architecture: `get_balance` queries TB or Scylla.
# UBSCore does NOT expose HTTP balance query directly.
# However, UBSCore emits BalanceEvent.Determined... Settlement writes to Scylla.
# So if we wait, Scylla `user_balances` might be updated?
# BUT SettlementService is not running in this script!
# We should start SettlementService if we want to query balance via API.
# OR we request Spot->Funding and check if it succeeds (which implies balance was sufficient).

echo "💸 Requesting Spot -> Funding Transfer..."
# API: POST /api/v1/user/internal_transfer
# Payload: { from_account: { type: "Spot", user_id: ...}, to_account: { type: "Funding", ...}, amount, asset }

RESPONSE=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {
        "account_type": "spot",
        "user_id": '$USER_ID',
        "asset": "'$ASSET'"
    },
    "to_account": {
        "account_type": "funding",
        "user_id": '$USER_ID',
        "asset": "'$ASSET'"
    },
    "amount": "0.5",
    "asset": "'$ASSET'"
  }')

echo "Response: $RESPONSE"

# Extract Request ID
REQ_ID=$(echo $RESPONSE | jq -r '.data.request_id')
echo "Request ID: $REQ_ID"

if [ "$REQ_ID" == "null" ]; then
    echo "❌ Failed to create transfer"
    exit 1
fi

# 3. Wait for Processing
echo "⏳ Waiting for settlement..."
sleep 5

cleanup() {
    echo "🧹 Stopping services..."
    if [ -n "$GATEWAY_PID" ]; then kill $GATEWAY_PID 2>/dev/null || true; fi
    if [ -n "$UBS_PID" ]; then kill $UBS_PID 2>/dev/null || true; fi
    pkill -f "target/debug/order_gate_server" || true
    pkill -f "target/debug/ubscore_aeron_service" || true
    rm -rf /tmp/aeron-*
}

check_status() {
    STATUS_RESP=$(curl -s http://localhost:3001/api/v1/user/internal_transfer/$REQ_ID)
    echo "Status Response: $STATUS_RESP"
    status=$(echo $STATUS_RESP | jq -r '.data.status')
}


# 4. Check Status
# Check status loop
for i in {1..60}; do
    check_status
    if [ "$status" == "success" ]; then
        echo "Transfer Status: success"
        echo "✅ Test Passed!"
        cleanup
        exit 0
    elif [ "$status" == "failed" ]; then
        echo "Transfer Status: failed"
        echo "❌ Test Failed. Status: failed"
        break
    fi
    echo "Transfer Status: $status (Attempt $i/60)"
    sleep 1
done

echo "❌ Test Failed. Transfer did not reach 'success' status within the timeout."
cat gateway.log
cat ubscore.log
cleanup
exit 1
