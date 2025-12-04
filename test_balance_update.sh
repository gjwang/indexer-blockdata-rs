#!/bin/bash

# Complete Balance Update Test with Order Flow

set -e

echo "=== Cleaning up ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
pkill -f order_gate_server || true
sleep 2

# Clear ME state to get fresh trades
echo "=== Clearing Matching Engine State ==="
rm -rf /tmp/matching_engine_data || true

echo ""
echo "=== Starting Matching Engine ==="
./target/debug/matching_engine_server > /tmp/me.log 2>&1 &
ME_PID=$!
sleep 5

echo ""
echo "=== Starting Settlement Service ==="
./target/debug/settlement_service > /tmp/settle.log 2>&1 &
SETTLE_PID=$!
sleep 3

echo ""
echo "=== Starting Order Gateway ==="
./target/debug/order_gate_server > /tmp/gateway.log 2>&1 &
GATEWAY_PID=$!
sleep 3

echo ""
echo "=== Sending Orders (5 seconds) ==="
timeout 5 ./target/debug/order_http_client > /tmp/client.log 2>&1 || true
sleep 2

echo ""
echo "=== Checking Settlement Logs (last 50 lines) ==="
tail -50 /tmp/settle.log | grep -E "(Seq:|Balances updated|Balance update skipped|Failed to update)" || echo "No balance update logs found"

echo ""
echo "=== Querying Settled Trades Count ==="
docker exec -i scylla cqlsh -e "SELECT COUNT(*) FROM settlement.settled_trades WHERE trade_date = $(date +%s | awk '{print int($1/86400)}');"

echo ""
echo "=== Querying Balance Updates ==="
docker exec -i scylla cqlsh -e "SELECT user_id, asset_id, available, version FROM settlement.user_balances LIMIT 20;"

echo ""
echo "=== Balance Update Statistics ==="
echo -n "Total balance updates: "
grep -c "Balances updated" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "Version conflicts: "
grep -c "Balance update skipped" /tmp/settle.log 2>/dev/null || echo "0"

echo ""
echo "=== Cleanup ==="
kill $ME_PID $SETTLE_PID $GATEWAY_PID 2>/dev/null || true

echo ""
echo "Done!"
