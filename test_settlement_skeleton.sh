#!/bin/bash
set -e

echo "=== Building binaries ==="
cargo build --bin matching_engine_server
cargo build --bin settlement_service
cargo build --bin order_http_client

echo "=== Cleanup ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
rm -rf me_wal_data me_snapshots

echo "=== Starting Settlement Service ==="
./target/debug/settlement_service > settlement.log 2>&1 &
SET_PID=$!
echo "Settlement PID: $SET_PID"

echo "=== Starting Matching Engine ==="
APP__ENABLE_LOCAL_WAL=false ./target/debug/matching_engine_server > me_server.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 5

echo "=== Sending Orders ==="
./target/debug/order_http_client > client.log 2>&1 || true
sleep 5

echo "=== Checking Settlement Output ==="
echo "Settlement Log:"
cat settlement.log
echo ""

echo "=== Checking for Sequence Numbers ==="
if grep -q "\[Settlement\] Seq:" settlement.log; then
    echo "SUCCESS: Settlement Sequence found in logs."
else
    echo "WARNING: Settlement Sequence not found in logs."
fi

if grep -q "GAP DETECTED" settlement.log; then
    echo "FAILURE: Sequence Gap Detected!"
    exit 1
else
    echo "SUCCESS: No Sequence Gaps detected."
fi

echo "=== Checking CSV Persistence ==="
if [ -f "settled_trades.csv" ]; then
    echo "SUCCESS: settled_trades.csv created."
    echo "First 5 lines of CSV:"
    head -n 5 settled_trades.csv
else
    echo "FAILURE: settled_trades.csv NOT found."
    exit 1
fi

echo "=== Cleanup ==="
kill $ME_PID $SET_PID 2>/dev/null || true
rm -f settled_trades.csv
