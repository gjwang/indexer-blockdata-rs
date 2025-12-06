#!/bin/bash

# Manual step-by-step test script

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

echo ""
echo "=== Step 4: Start Services ==="
echo "Starting Settlement Service..."
./target/release/settlement_service > logs/settlement.log 2>&1 &
SETTLEMENT_PID=$!
echo "Settlement PID: $SETTLEMENT_PID"
sleep 3

echo ""
echo "Starting Matching Engine..."
./target/release/matching_engine_server > logs/matching_engine.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 5

echo ""
echo "Starting Order Gateway..."
./target/debug/order_gate_server > logs/order_gateway.log 2>&1 &
GW_PID=$!
echo "Order Gateway PID: $GW_PID"
sleep 2

echo ""
echo "=== Services Started ==="
echo "Press ENTER to send Deposit 1 (User 1001)..."
read

echo ""
echo "=== Step 5: Send Deposit 1 (User 1001) ==="
TS=$(date +%s%3N)
echo "{\"type\":\"TransferIn\",\"data\":{\"request_id\":\"req-1\",\"user_id\":1001,\"asset_id\":1,\"amount\":100000000,\"timestamp\":$TS}}" | \
  docker exec -i redpanda rpk topic produce balance.operations

echo "Deposit 1 sent. Waiting 5s for processing..."
sleep 5

echo ""
echo "=== Check Settlement Log for Deposit 1 ==="
tail -n 20 logs/settlement.log | grep -E "Deposit|Lock|message #"

echo ""
echo "Press ENTER to send Deposit 2 (User 1002)..."
read

echo ""
echo "=== Step 6: Send Deposit 2 (User 1002) ==="
TS=$(date +%s%3N)
echo "{\"type\":\"TransferIn\",\"data\":{\"request_id\":\"req-2\",\"user_id\":1002,\"asset_id\":2,\"amount\":2000000000000,\"timestamp\":$TS}}" | \
  docker exec -i redpanda rpk topic produce balance.operations

echo "Deposit 2 sent. Waiting 5s for processing..."
sleep 5

echo ""
echo "=== Check Settlement Log for Deposit 2 ==="
tail -n 30 logs/settlement.log | grep -E "Deposit|Lock|message #"

echo ""
echo "=== Step 7: Check Balances in DB ==="
docker exec scylla cqlsh -k trading -e "SELECT * FROM user_balances;"

echo ""
echo "Press ENTER to place orders..."
read

echo ""
echo "=== Step 8: Place Orders ==="
# Sell order
curl -X POST "http://127.0.0.1:3001/api/orders?user_id=1001" \
  -H "Content-Type: application/json" \
  -d '{"cid":"sell_manual_1001","symbol":"BTC_USDT","side":"Sell","order_type":"Limit","price":"150.0","quantity":"0.5"}'

echo ""
sleep 1

# Buy order
curl -X POST "http://127.0.0.1:3001/api/orders?user_id=1002" \
  -H "Content-Type: application/json" \
  -d '{"cid":"buy_manual_1002","symbol":"BTC_USDT","side":"Buy","order_type":"Limit","price":"150.0","quantity":"0.5"}'

echo ""
echo "Orders placed. Waiting 5s for settlement..."
sleep 5

echo ""
echo "=== Step 9: Check Final State ==="
echo "Settlement Log (last 50 lines):"
tail -n 50 logs/settlement.log | grep -E "Deposit|Lock|Settled|Failed"

echo ""
echo "User Balances:"
docker exec scylla cqlsh -k trading -e "SELECT * FROM user_balances;"

echo ""
echo "Settled Trades:"
docker exec scylla cqlsh -k trading -e "SELECT * FROM settled_trades;"

echo ""
echo "Order History:"
docker exec scylla cqlsh -k trading -e "SELECT order_id, side, status, filled_qty FROM order_history;"

echo ""
echo "=== Test Complete ==="
echo "Settlement PID: $SETTLEMENT_PID"
echo "Matching Engine PID: $ME_PID"
echo "Order Gateway PID: $GW_PID"
echo ""
echo "To kill services: pkill -f matching_engine_server; pkill -f settlement_service; pkill -f order_gate_server"
