#!/bin/bash

# Clean all data and run fresh E2E test

set -e

echo "=========================================="
echo "  Clean and Fresh E2E Test"
echo "=========================================="

echo ""
echo "=== Step 1: Killing all processes ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
pkill -f order_gate_server || true
pkill -f order_http_client || true
sleep 2

echo ""
echo "=== Step 2: Cleaning Matching Engine data ==="
rm -rf me_wal_data || true
rm -rf me_snapshots || true
rm -f settled_trades.csv || true
rm -f failed_trades.json || true
echo "✅ ME data cleaned"

echo ""
echo "=== Step 3: Cleaning ScyllaDB data ==="
docker exec -i scylla cqlsh -e "TRUNCATE settlement.settled_trades;" 2>/dev/null || echo "⚠️  settled_trades table may not exist"
docker exec -i scylla cqlsh -e "TRUNCATE settlement.user_balances;" 2>/dev/null || echo "⚠️  user_balances table may not exist"
docker exec -i scylla cqlsh -e "TRUNCATE settlement.ledger_events;" 2>/dev/null || echo "⚠️  ledger_events table may not exist"
echo "✅ ScyllaDB data cleaned"

echo ""
echo "=== Step 4: Cleaning log files ==="
rm -f /tmp/me.log /tmp/settle.log /tmp/gateway.log /tmp/client.log
echo "✅ Log files cleaned"

echo ""
echo "=== Step 5: Building binaries ==="
cargo build --bin matching_engine_server --bin settlement_service --bin order_gate_server --bin order_http_client 2>&1 | tail -5

echo ""
echo "=== Step 6: Starting services in order ==="

echo "  6.1 Starting Settlement Service first..."
./target/debug/settlement_service > /tmp/settle.log 2>&1 &
SETTLE_PID=$!
sleep 3
echo "      Settlement PID: $SETTLE_PID"

echo "  6.2 Starting Matching Engine..."
./target/debug/matching_engine_server > /tmp/me.log 2>&1 &
ME_PID=$!
sleep 5
echo "      ME PID: $ME_PID"

echo "  6.3 Starting Order Gateway..."
./target/debug/order_gate_server > /tmp/gateway.log 2>&1 &
GATEWAY_PID=$!
sleep 3
echo "      Gateway PID: $GATEWAY_PID"

echo ""
echo "=== Step 7: Verifying services are running ==="
ps -p $SETTLE_PID > /dev/null && echo "  ✅ Settlement running" || echo "  ❌ Settlement died"
ps -p $ME_PID > /dev/null && echo "  ✅ ME running" || echo "  ❌ ME died"
ps -p $GATEWAY_PID > /dev/null && echo "  ✅ Gateway running" || echo "  ❌ Gateway died"

echo ""
echo "=== Step 8: Sending test orders (10 seconds) ==="
timeout 10 ./target/debug/order_http_client > /tmp/client.log 2>&1 || true
echo "  ✅ Orders sent"

echo ""
echo "=== Step 9: Waiting for settlement (5 seconds) ==="
sleep 5

echo ""
echo "=========================================="
echo "  Test Results"
echo "=========================================="

echo ""
echo "=== Matching Engine Stats ==="
tail -20 /tmp/me.log | grep -E "(PERF|Match|Output)" | tail -10

echo ""
echo "=== Settlement Service Stats ==="
echo "Last 30 lines of settlement log:"
tail -30 /tmp/settle.log

echo ""
echo "=== Database Verification ==="

echo ""
echo "1. Settled Trades Count:"
docker exec -i scylla cqlsh -e "SELECT COUNT(*) FROM settlement.settled_trades;" 2>/dev/null

echo ""
echo "2. User Balances (first 20):"
docker exec -i scylla cqlsh -e "SELECT user_id, asset_id, available, frozen, version FROM settlement.user_balances LIMIT 20;" 2>/dev/null

echo ""
echo "3. Balance Update Statistics:"
echo -n "   Total balance updates: "
grep -c "Balances updated" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "   Version conflicts: "
grep -c "Balance update skipped" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "   Balance update errors: "
grep -c "Failed to update balances" /tmp/settle.log 2>/dev/null || echo "0"

echo ""
echo "=== Sample Balance Update Logs ==="
grep -E "(Balances updated|Balance update skipped|Failed to update)" /tmp/settle.log 2>/dev/null | head -10 || echo "No balance update logs found"

echo ""
echo "=========================================="
echo "  Cleanup"
echo "=========================================="
kill $ME_PID $SETTLE_PID $GATEWAY_PID 2>/dev/null || true
sleep 1

echo ""
echo "✅ Test Complete!"
echo ""
echo "Summary:"
echo "  - Check 'Settled Trades Count' should be > 0"
echo "  - Check 'User Balances' should show entries with versions"
echo "  - Check 'Balance Update Statistics' for success rate"
