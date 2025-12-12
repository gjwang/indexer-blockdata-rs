#!/bin/bash
# tests/14_rescue_transfer_e2e.sh
set -e

echo "🧪 RESCUE OPERATION E2E TEST"

export APP_ENV=dev
export KAFKA_BROKERS=localhost:9093
export KAFKA_GROUP_ID="ubscore_rescue_${RANDOM}"

# 1. Kill everything
pkill -f "order_gate_server" || true
pkill -f "ubscore_aeron_service" || true
pkill -f "internal_transfer_settlement" || true

# Clean topics
echo "🧹 Cleaning Kafka topics..."
docker exec redpanda rpk topic delete balance.operations balance.events || true
sleep 3
docker exec redpanda rpk topic create balance.operations balance.events -p 1 || true

# 2. Build binaries (ensure updated)
cargo build --bin order_gate_server
cargo build --bin ubscore_aeron_service
cargo build --bin internal_transfer_settlement

# 3. Start Gateway & UBSCore (BUT NOT SETTLEMENT)
echo "Starting UBSCore..."
RUST_LOG=info ./target/debug/ubscore_aeron_service > ubscore.log 2>&1 &
UBS_PID=$!
sleep 5

echo "Starting Gateway..."
RUST_LOG=info ./target/debug/order_gate_server > gateway.log 2>&1 &
GATEWAY_PID=$!
sleep 5

# 4. Fund the account (Deposit)
USER_ID=3001
echo "💰 Funding User $USER_ID..."
curl -s -X POST http://localhost:3001/api/v1/user/transfer_in \
  -H "Content-Type: application/json" \
  -d '{"request_id":"fund_01","user_id":'$USER_ID',"asset":"USDT","amount":"1000.0"}'

sleep 2

# 5. Initiate Spot->Funding Transfer
echo "💸 Initiating Transfer (Spot -> Funding)..."
RESP=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {"account_type": "spot", "user_id": '$USER_ID', "asset": "USDT"},
    "to_account": {"account_type": "funding", "user_id": '$USER_ID', "asset": "USDT"},
    "amount": "100.0"
  }')

echo "Response: $RESP"

REQ_ID=$(echo $RESP | jq -r '.data.request_id')
echo "Request ID: $REQ_ID"

if [ "$REQ_ID" == "null" ]; then
    echo "Failed to verify request creation"
    cat gateway.log
    exit 1
fi

# 6. Verify Status is STUCK at Pending
echo "⏳ Verifying STUCK state..."
sleep 5
STATUS_RESP=$(curl -s http://localhost:3001/api/v1/user/internal_transfer/$REQ_ID)
STATUS=$(echo $STATUS_RESP | jq -r '.data.status')
echo "Current Status: $STATUS"

if [ "$STATUS" != "pending" ]; then
    echo "❌ Unexpected status: $STATUS (Expected: pending)"
    exit 1
fi

echo "✅ Transfer confirmed stuck in Pending state."

# 7. LAUNCH THE RESCUE SERVICE (With FRESH Group ID and Correct Port)
echo "🚀 LAUNCHING INTERNAL TRANSFER SETTLEMENT SERVICE..."
export KAFKA_GROUP_ID="settlement_rescue_${RANDOM}"
export KAFKA_BROKERS="localhost:9093"
RUST_LOG=info ./target/debug/internal_transfer_settlement > settlement.log 2>&1 &
SETTLEMENT_PID=$!

# 8. Poll for Success
echo "👀 Watching for Recovery..."
for i in {1..30}; do
    STATUS_RESP=$(curl -s http://localhost:3001/api/v1/user/internal_transfer/$REQ_ID)
    STATUS=$(echo $STATUS_RESP | jq -r '.data.status')

    if [ "$STATUS" == "success" ]; then
        echo "🎉 RESCUE SUCCESSFUL! Status is now: $STATUS"
        break
    fi

    echo "Still $STATUS... ($i/30)"
    sleep 2
done

if [ "$STATUS" != "success" ]; then
    echo "❌ RESCUE FAILED."
    echo "Settlement Logs:"
    cat settlement.log
    exit 1
fi

# Cleanup
kill $GATEWAY_PID $UBS_PID $SETTLEMENT_PID 2>/dev/null || true
echo "✅ Verification Complete"
exit 0
