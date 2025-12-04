#!/bin/bash
# End-to-End Test for Settlement Service with ScyllaDB
set -e

echo "=== Settlement Service E2E Test with ScyllaDB ==="
echo ""

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

success() {
    echo -e "${GREEN}✅ $1${NC}"
}

failure() {
    echo -e "${RED}❌ $1${NC}"
}

warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

# Check if ScyllaDB is running
echo "=== Checking Prerequisites ==="
if ! docker ps | grep -q scylla; then
    failure "ScyllaDB is not running!"
    echo "Start it with: docker-compose up -d scylla"
    exit 1
fi
success "ScyllaDB is running"

# Check if schema is initialized
if ! docker exec scylla cqlsh -e "DESCRIBE KEYSPACE settlement" > /dev/null 2>&1; then
    failure "ScyllaDB schema not initialized!"
    echo "Initialize it with: ./scripts/init_scylla.sh"
    exit 1
fi
success "ScyllaDB schema is initialized"

# Build binaries
echo ""
echo "=== Building Binaries ==="
cargo build --bin matching_engine_server --quiet
cargo build --bin settlement_service --quiet
cargo build --bin order_http_client --quiet
success "Binaries built"

# Cleanup
echo ""
echo "=== Cleanup ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
rm -rf me_wal_data me_snapshots
rm -f settlement.log me_server.log client.log settled_trades.csv
success "Cleanup complete"

# Clear ScyllaDB data
echo ""
echo "=== Clearing ScyllaDB Data ==="
docker exec scylla cqlsh -e "TRUNCATE settlement.settled_trades;" > /dev/null 2>&1 || true
success "ScyllaDB data cleared"

# Start Settlement Service
echo ""
echo "=== Starting Settlement Service ==="
./target/debug/settlement_service > settlement.log 2>&1 &
SET_PID=$!
echo "Settlement PID: $SET_PID"

# Wait for settlement service to connect to ScyllaDB
sleep 3

# Check if settlement service started successfully
if ! ps -p $SET_PID > /dev/null; then
    failure "Settlement service failed to start!"
    echo "Log output:"
    cat settlement.log
    exit 1
fi

if grep -q "Connected to ScyllaDB" settlement.log; then
    success "Settlement service connected to ScyllaDB"
else
    failure "Settlement service did not connect to ScyllaDB"
    echo "Log output:"
    cat settlement.log
    kill $SET_PID 2>/dev/null || true
    exit 1
fi

# Start Matching Engine
echo ""
echo "=== Starting Matching Engine ==="
APP__ENABLE_LOCAL_WAL=false ./target/debug/matching_engine_server > me_server.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 5

# Send Orders
echo ""
echo "=== Sending Orders ==="
./target/debug/order_http_client > client.log 2>&1 || true
sleep 5

# Verification
echo ""
echo "=== Verification ==="

# 1. Check Settlement Logs
echo ""
echo "--- Settlement Service Logs ---"
if grep -q "Seq:" settlement.log; then
    success "Settlement sequences found in logs"
    echo "Sample log entries:"
    grep "Seq:" settlement.log | head -n 3
else
    failure "Settlement sequences NOT found in logs"
    echo "Full settlement log:"
    cat settlement.log
    kill $ME_PID $SET_PID 2>/dev/null || true
    exit 1
fi

# 2. Check for Gaps
echo ""
if grep -q "GAP DETECTED" settlement.log; then
    failure "Sequence Gap Detected!"
    grep "GAP DETECTED" settlement.log
    kill $ME_PID $SET_PID 2>/dev/null || true
    exit 1
else
    success "No sequence gaps detected"
fi

# 3. Check CSV Persistence (backup)
echo ""
if [ -f "settled_trades.csv" ]; then
    success "CSV backup file created"
    CSV_COUNT=$(wc -l < settled_trades.csv)
    echo "CSV contains $CSV_COUNT trades"
else
    failure "CSV backup file NOT found"
    kill $ME_PID $SET_PID 2>/dev/null || true
    exit 1
fi

# 4. Check ScyllaDB Persistence (primary)
echo ""
echo "--- ScyllaDB Verification ---"
# Use awk to extract the number, handling potential whitespace
# grep -o '[0-9]\+' extracts all numbers. The first one is the count, the second is "(1 rows)"
DB_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM settlement.settled_trades;" | grep -o '[0-9]\+' | head -n 1)

if [ -z "$DB_COUNT" ]; then
    failure "Could not query ScyllaDB"
    kill $ME_PID $SET_PID 2>/dev/null || true
    exit 1
fi

if [ "$DB_COUNT" -gt 0 ]; then
    success "ScyllaDB contains $DB_COUNT trades"
else
    failure "ScyllaDB contains 0 trades!"
    kill $ME_PID $SET_PID 2>/dev/null || true
    exit 1
fi

# 5. Verify data consistency
echo ""
if [ "$DB_COUNT" -eq "$CSV_COUNT" ]; then
    success "Data consistency verified (DB: $DB_COUNT, CSV: $CSV_COUNT)"
else
    warning "Data mismatch! DB: $DB_COUNT, CSV: $CSV_COUNT"
fi

# 6. Sample data from ScyllaDB
echo ""
echo "--- Sample Trades from ScyllaDB ---"
docker exec scylla cqlsh -e "SELECT trade_id, output_sequence, price, quantity FROM settlement.settled_trades LIMIT 3;" 2>/dev/null || true

# 7. Check for errors
echo ""
if grep -q "Failed to insert trade to ScyllaDB" settlement.log; then
    failure "Database insertion errors detected!"
    grep "Failed to insert" settlement.log
    kill $ME_PID $SET_PID 2>/dev/null || true
    exit 1
else
    success "No database insertion errors"
fi

# Cleanup
echo ""
echo "=== Cleanup ==="
kill $ME_PID $SET_PID 2>/dev/null || true
sleep 1

# Keep data for inspection
echo ""
echo "=== Test Data Preserved ==="
echo "  - settlement.log (settlement service logs)"
echo "  - me_server.log (matching engine logs)"
echo "  - settled_trades.csv (CSV backup)"
echo "  - ScyllaDB data (query with: docker exec scylla cqlsh)"
echo ""

# Summary
echo "=== Test Summary ==="
success "All tests passed!"
echo ""
echo "To query the data:"
echo "  docker exec -it scylla cqlsh"
echo "  USE settlement;"
echo "  SELECT * FROM settled_trades LIMIT 10;"
echo ""
