#!/bin/bash

# Automated step-by-step test script (no pauses)

echo "=== Step 1: Clean Environment ==="
pkill -f matching_engine_server
pkill -f settlement_service
pkill -f order_gate_server
sleep 1

rm -rf wal snapshots

echo ""
echo "=== Step 2: Reset Database ==="
docker exec scylla cqlsh -e "DROP KEYSPACE IF EXISTS trading;"
docker exec scylla cqlsh -e "CREATE KEYSPACE trading WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};"
docker exec -i scylla cqlsh -k trading < schema/settlement_unified.cql

echo ""
echo "=== Step 3: Reset Kafka ==="
docker exec redpanda rpk topic delete orders trades balance.operations 2>/dev/null || true
docker exec redpanda rpk topic create orders trades balance.operations -p 1 -r 1
sleep 3

echo ""
echo "=== Step 4: Start Services ==="
echo "Starting Settlement Service..."
./target/release/settlement_service > logs/settlement.log 2>&1 &
SETTLEMENT_PID=$!
echo "Settlement PID: $SETTLEMENT_PID"
sleep 5

echo ""
echo "Starting Matching Engine..."
./target/release/matching_engine_server > logs/matching_engine.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 10

echo ""
echo "Starting Order Gateway..."
./target/debug/order_gate_server > logs/order_gateway.log 2>&1 &
GW_PID=$!
echo "Order Gateway PID: $GW_PID"
sleep 2

echo ""
echo "=== Step 5: Send Deposit 1 (User 1001, Asset 1, 100M) ==="
TS=$(python3 -c 'import time; print(int(time.time() * 1000))')
echo "{\"type\":\"TransferIn\",\"data\":{\"request_id\":\"req-1\",\"user_id\":1001,\"asset_id\":1,\"amount\":100000000,\"timestamp\":$TS}}" | \
  docker exec -i redpanda rpk topic produce balance.operations

echo "Deposit 1 sent. Waiting 3s for processing..."
sleep 3

echo ""
echo "=== ME Log - Check if Deposit 1 was sent ==="
grep "ZMQ.*Publishing Deposit.*1001" logs/matching_engine.log || echo "NOT FOUND in ME log"

echo ""
echo "=== Settlement Log - Check if Deposit 1 was received ==="
grep "Deposit User 1001" logs/settlement.log || echo "NOT FOUND in Settlement log"

echo ""
echo "=== Step 6: Send Deposit 2 (User 1002, Asset 2, 2T) ==="
TS=$(python3 -c 'import time; print(int(time.time() * 1000))')
echo "{\"type\":\"TransferIn\",\"data\":{\"request_id\":\"req-2\",\"user_id\":1002,\"asset_id\":2,\"amount\":2000000000000,\"timestamp\":$TS}}" | \
  docker exec -i redpanda rpk topic produce balance.operations

echo "Deposit 2 sent. Waiting 3s for processing..."
sleep 3

echo ""
echo "=== ME Log - Check if Deposit 2 was sent ==="
grep "ZMQ.*Publishing Deposit.*1002" logs/matching_engine.log || echo "NOT FOUND in ME log"

echo ""
echo "=== Settlement Log - Check if Deposit 2 was received ==="
grep "Deposit User 1002" logs/settlement.log || echo "NOT FOUND in Settlement log"

echo ""
echo "=== Step 7: Check Balances in DB ==="
docker exec scylla cqlsh -k trading -e "SELECT user_id, asset_id, avail, version FROM user_balances;"

echo ""
echo "=== Step 8: Place Orders ==="
# Sell order
curl -s -X POST "http://127.0.0.1:3001/api/orders?user_id=1001" \
  -H "Content-Type: application/json" \
  -d '{"cid":"sell_auto_1001","symbol":"BTC_USDT","side":"Sell","order_type":"Limit","price":"150.0","quantity":"0.5"}' | jq .

sleep 1

# Buy order
curl -s -X POST "http://127.0.0.1:3001/api/orders?user_id=1002" \
  -H "Content-Type: application/json" \
  -d '{"cid":"buy_auto_1002","symbol":"BTC_USDT","side":"Buy","order_type":"Limit","price":"150.0","quantity":"0.5"}' | jq .

echo ""
echo "Orders placed. Waiting 5s for settlement..."
sleep 5

echo ""
echo "=== Step 9: Final State ==="
echo ""
echo "All ZMQ messages sent by ME:"
grep "ZMQ.*Publishing" logs/matching_engine.log

echo ""
echo "All Deposits/Locks received by Settlement:"
grep -E "Deposit User|Lock User" logs/settlement.log

echo ""
echo "User Balances:"
docker exec scylla cqlsh -k trading -e "SELECT user_id, asset_id, avail, frozen, version FROM user_balances;"

echo ""
echo "Settled Trades:"
docker exec scylla cqlsh -k trading -e "SELECT trade_id, price, quantity FROM settled_trades;"

echo ""
echo "Order History:"
docker exec scylla cqlsh -k trading -e "SELECT order_id, side, status, filled_qty FROM order_history;"

echo ""
echo "=== Test Complete ==="
echo ""
echo "To kill services: pkill -f matching_engine_server; pkill -f settlement_service; pkill -f order_gate_server"
