#!/bin/bash
# tests/15_funding_spot_direct_e2e.sh
# Verification of Funding -> Spot (Direct/Generic Path)

set -e
echo "🧪 BIDIRECTIONAL DIRECT TRANSFER TEST (Spot <-> Funding)"
echo "🧪 FUNDING -> SPOT DIRECT TRANSFER TEST"

export APP_ENV=dev
export KAFKA_BROKERS=localhost:9093
export KAFKA_GROUP_ID="ubscore_test15_${RANDOM}"

cleanup() {
    echo "🧹 Cleanup..."
    pkill -f "order_gate_server" || true
    pkill -f "ubscore_aeron_service" || true
    pkill -f "internal_transfer_settlement" || true
}

# 1. Kill everything
cleanup

# Clean topics
echo "🧹 Cleaning Kafka topics..."
docker exec redpanda rpk topic delete balance.operations balance.events || true
sleep 3
docker exec redpanda rpk topic create balance.operations balance.events -p 1 || true

# 3. Start Services
echo "Starting UBSCore..."
RUST_LOG=info ./target/debug/ubscore_aeron_service > ubscore.log 2>&1 &
UBS_PID=$!
sleep 5

echo "Starting Gateway..."
RUST_LOG=info ./target/debug/order_gate_server > gateway.log 2>&1 &
GATEWAY_PID=$!
sleep 5

echo "Starting Settlement..."
# Use its own group
export KAFKA_GROUP_ID="settlement_test15_${RANDOM}"
RUST_LOG=info ./target/debug/internal_transfer_settlement > settlement.log 2>&1 &
SETTLEMENT_PID=$!
sleep 5

# 4. Setup Funding
USER_ID=4001
ASSET="BTC"
echo "💰 Step 1: Deposit to Spot ($USER_ID)..."
curl -s -X POST http://localhost:3001/api/v1/user/transfer_in \
  -H "Content-Type: application/json" \
  -d '{"request_id":"setup_15","user_id":'$USER_ID',"asset":"'$ASSET'","amount":"2.0"}'
sleep 2

echo "💸 Step 2: Spot -> Funding (1.0)..."
RESP=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {"account_type": "spot", "user_id": '$USER_ID', "asset": "'$ASSET'"},
    "to_account": {"account_type": "funding", "user_id": '$USER_ID', "asset": "'$ASSET'"},
    "amount": "1.0"
  }')
REQ_ID_1=$(echo $RESP | jq -r '.data.request_id')
echo "Req 1 ID: $REQ_ID_1"

# Wait for readiness
sleep 5

# 5. Test Funding -> Spot
echo "🔄 Step 3: Funding -> Spot (0.5)..."
RESP_2=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {"account_type": "funding", "user_id": '$USER_ID', "asset": "'$ASSET'"},
    "to_account": {"account_type": "spot", "user_id": '$USER_ID', "asset": "'$ASSET'"},
    "amount": "0.5"
  }')

echo "Response 2: $RESP_2"
REQ_ID_2=$(echo $RESP_2 | jq -r '.data.request_id')
STATUS=$(echo $RESP_2 | jq -r '.data.status')
echo "Req 2 ID: $REQ_ID_2"
echo "Initial Status: $STATUS"

if [ "$STATUS" != "pending" ]; then
   echo "Note: Initial status is $STATUS"
fi

# Poll for final status
echo "👀 Polling for Req 2 Success..."
for i in {1..20}; do
    STATUS_RESP=$(curl -s http://localhost:3001/api/v1/user/internal_transfer/$REQ_ID_2)
    CUR_STATUS=$(echo $STATUS_RESP | jq -r '.data.status')

    if [ "$CUR_STATUS" == "success" ]; then
        echo "✅ Funding -> Spot Succeeded!"
        cleanup
        exit 0
    fi
    echo "Status: $CUR_STATUS ($i/20)"
    sleep 2
done

echo "❌ Failed: Funding -> Spot Generic Path stuck in $CUR_STATUS"
cleanup
exit 1
