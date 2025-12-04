#!/bin/bash

# Full E2E Test Script for Settlement with Balance Updates

set -e

echo "=== Building all binaries (DEBUG) ==="
cargo build --bin matching_engine_server
cargo build --bin settlement_service
cargo build --bin order_http_client
cargo build --bin order_gate_server

echo ""
echo "=== Killing any existing processes ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
pkill -f order_http_client || true
pkill -f order_gate_server || true
sleep 2

echo ""
echo "=== Clearing old data ==="
rm -f /tmp/matching_engine.log
rm -f /tmp/settlement.log
rm -f /tmp/order_gate.log
rm -f /tmp/order_client.log

echo ""
echo "=== Starting Matching Engine Server ==="
./target/debug/matching_engine_server > /tmp/matching_engine.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 5

echo ""
echo "=== Starting Settlement Service ==="
./target/debug/settlement_service > /tmp/settlement.log 2>&1 &
SETTLE_PID=$!
echo "Settlement Service PID: $SETTLE_PID"
sleep 3

echo ""
echo "=== Starting Order Gateway ==="
./target/debug/order_gate_server > /tmp/order_gate.log 2>&1 &
OG_PID=$!
echo "Order Gateway PID: $OG_PID"
sleep 5

echo ""
echo "=== Sending Test Orders (10 seconds) ==="
perl -e 'alarm 10; exec @ARGV' "./target/debug/order_http_client" > /tmp/order_client.log 2>&1 || true
sleep 3

echo ""
echo "=== Checking Matching Engine Logs ==="
echo "Last 20 lines:"
tail -20 /tmp/matching_engine.log

echo ""
echo "=== Checking Settlement Service Logs ==="
echo "Last 30 lines:"
tail -30 /tmp/settlement.log

echo ""
echo "=== Querying ScyllaDB for Settled Trades ==="
docker exec -i scylla cqlsh -e "SELECT COUNT(*) FROM settlement.settled_trades;" 2>/dev/null || echo "Failed to query trades"

echo ""
echo "=== Querying ScyllaDB for User Balances ==="
docker exec -i scylla cqlsh -e "SELECT user_id, asset_id, available, frozen, version FROM settlement.user_balances LIMIT 10;" 2>/dev/null || echo "Failed to query balances"

echo ""
echo "=== Balance Update Statistics ==="
grep -c "Balances updated" /tmp/settlement.log || echo "0 balance updates"
grep -c "Balance update skipped" /tmp/settlement.log || echo "0 version conflicts"

echo ""
echo "=== Cleanup ==="
kill $ME_PID $SETTLE_PID $OG_PID 2>/dev/null || true

echo ""
echo "=== Test Complete ==="
echo "Check logs above for:"
echo "  - Trade settlement in /tmp/settlement.log"
echo "  - Balance updates in ScyllaDB"
echo "  - Version conflict rate (should be low)"
