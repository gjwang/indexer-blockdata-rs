#!/bin/bash

# End-to-End Test Script for Matching Engine

set -e

echo "=== Building all binaries ==="
cargo build --release --bin matching_engine_server
cargo build --release --bin trade_history_consumer
cargo build --release --bin order_http_client
cargo build --release --bin order_gate_server

echo ""
echo "=== Killing any existing processes ==="
pkill -f matching_engine_server || true
pkill -f trade_history_consumer || true
pkill -f order_http_client || true
pkill -f order_gate_server || true
sleep 2

echo ""
echo "=== Starting Matching Engine Server ==="
./target/release/matching_engine_server > /tmp/matching_engine.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 5

echo ""
echo "=== Starting Trade History Consumer ==="
./target/release/trade_history_consumer > /tmp/trade_consumer.log 2>&1 &
TC_PID=$!
echo "Trade Consumer PID: $TC_PID"
sleep 2

echo ""
echo "=== Starting Order Gateway ==="
./target/release/order_gate_server > /tmp/order_gate.log 2>&1 &
OG_PID=$!
echo "Order Gateway PID: $OG_PID"
sleep 5

echo ""
echo "=== Sending Test Orders ==="
# Run client for 10 seconds then kill it (using perl for cross-platform timeout)
perl -e 'alarm 10; exec @ARGV' "./target/release/order_http_client" > /tmp/order_client.log 2>&1 || true
sleep 3

echo ""
echo "=== Checking Matching Engine Logs ==="
echo "Last 30 lines:"
tail -30 /tmp/matching_engine.log

echo ""
echo "=== Checking Trade Consumer Logs ==="
echo "Last 20 lines:"
tail -20 /tmp/trade_consumer.log

echo ""
echo "=== Cleanup ==="
kill $ME_PID $TC_PID $OG_PID 2>/dev/null || true

echo ""
echo "=== Test Complete ==="
echo "Check logs above for:"
echo "  - [WAL+Match] logs in matching engine"
echo "  - Trade messages in consumer"
