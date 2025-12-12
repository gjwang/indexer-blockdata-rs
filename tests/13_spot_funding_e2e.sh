#!/bin/bash
# 13_spot_funding_e2e.sh
# E2E Test for Spot -> Funding internal transfers via UBSCore

# Cleanup previous runs
pkill -f "target/debug/order_gate_server" || true
pkill -f "target/debug/ubscore_aeron_service" || true
pkill -f "target/debug/internal_transfer_settlement" || true

set -e

echo "🚀 Starting Spot -> Funding E2E Test"
export APP_ENV=dev
export KAFKA_BROKERS=localhost:9093
export KAFKA_GROUP_ID="ubscore_e2e_run_${RANDOM}"
echo "Configured KAFKA_GROUP_ID: $KAFKA_GROUP_ID"

# 1. Start Services
pkill -f "order_gate_server" || true
pkill -f "ubscore_aeron_service" || true
pkill -f "internal_transfer_settlement" || true

# Check Kafka/Scylla/TB are up
docker compose ps || true

echo "🛠 Creating Kafka topics..."
docker exec redpanda rpk topic delete balance.operations balance.events || true
sleep 3
docker exec redpanda rpk topic create balance.operations balance.events -p 1 || true

# Build binaries
cargo build --bin order_gate_server
cargo build --bin ubscore_aeron_service
cargo build --bin internal_transfer_settlement

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

# Start Internal Transfer Settlement Service
echo "Starting Internal Transfer Settlement Service..."
export KAFKA_GROUP_ID="settlement_service_e2e"
RUST_LOG=info ./target/debug/internal_transfer_settlement > settlement.log 2>&1 &
SETTLEMENT_PID=$!
echo "Settlement PID: $SETTLEMENT_PID"

sleep 5

# 2. Setup Test Data
USER_ID=999
ASSET="BTC"
AMOUNT="1.5"

echo "💰 Funding Spot Account via Transfer-In (Funding -> Spot)..."
curl -X POST http://localhost:3001/api/v1/user/transfer_in \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "setup_001",
    "user_id": '$USER_ID',
    "asset": "'$ASSET'",
    "amount": "'$AMOUNT'"
  }'

sleep 2

echo "💸 Requesting Spot -> Funding Transfer..."
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

REQ_ID=$(echo $RESPONSE | jq -r '.data.request_id')
echo "Request ID: $REQ_ID"

if [ "$REQ_ID" == "null" ]; then
    echo "❌ Failed to create transfer"
    exit 1
fi

echo "⏳ Waiting for settlement..."
sleep 5

cleanup() {
    echo "🧹 Stopping services..."
    if [ -n "$GATEWAY_PID" ]; then kill $GATEWAY_PID 2>/dev/null || true; fi
    if [ -n "$UBS_PID" ]; then kill $UBS_PID 2>/dev/null || true; fi
    if [ -n "$SETTLEMENT_PID" ]; then kill $SETTLEMENT_PID 2>/dev/null || true; fi
    pkill -f "target/debug/order_gate_server" || true
    pkill -f "target/debug/ubscore_aeron_service" || true
    pkill -f "target/debug/internal_transfer_settlement" || true
    rm -rf /tmp/aeron-*
}

check_status() {
    STATUS_RESP=$(curl -s http://localhost:3001/api/v1/user/internal_transfer/$REQ_ID)
    echo "Status Response: $STATUS_RESP"
    status=$(echo $STATUS_RESP | jq -r '.data.status')
}


# 4. Check Status
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
echo "=== Gateway Log ==="
cat gateway.log
echo "=== UBSCore Log ==="
cat ubscore.log
echo "=== Settlement Log ==="
cat settlement.log
cleanup
exit 1
