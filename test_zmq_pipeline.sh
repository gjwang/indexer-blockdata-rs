#!/bin/bash
set -e

echo "=== Building binaries ==="
cargo build --bin matching_engine_server
cargo build --bin zmq_subscriber_demo
cargo build --bin order_http_client

echo "=== Cleanup ==="
pkill -f matching_engine_server || true
pkill -f zmq_subscriber_demo || true
rm -rf me_wal_data me_snapshots

echo "=== Starting ZMQ Subscriber ==="
./target/debug/zmq_subscriber_demo > zmq_sub.log 2>&1 &
SUB_PID=$!
echo "Subscriber PID: $SUB_PID"

echo "=== Starting Matching Engine ==="
# Use APP__ENABLE_LOCAL_WAL=false to run in pure memory mode (optional, but good for speed)
APP__ENABLE_LOCAL_WAL=false ./target/debug/matching_engine_server > me_server.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 5

echo "=== Sending Orders ==="
# Send orders to trigger trades
# order_http_client sends a fixed sequence of orders that result in trades
./target/debug/order_http_client > client.log 2>&1 || true
sleep 5

echo "=== Checking ZMQ Output ==="
echo "Subscriber Log:"
cat zmq_sub.log
echo ""

echo "=== Checking for Sequence Numbers ==="
if grep -q "output_sequence" zmq_sub.log; then
    echo "SUCCESS: output_sequence found in logs."
    grep "output_sequence" zmq_sub.log | head -n 5
else
    echo "WARNING: output_sequence not found in logs."
fi

echo "=== Cleanup ==="
kill $ME_PID $SUB_PID 2>/dev/null || true
