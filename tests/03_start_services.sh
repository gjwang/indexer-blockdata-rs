#!/bin/bash
set -e

# Test 03: Start Services (Infrastructure + UBSCore + Gateway)
# This test starts all necessary services and keeps them running for subsequent tests

echo "🧪 TEST 03: Start Services"
echo "=========================="

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Check if services are already running
if pgrep -f "ubscore_aeron_service" > /dev/null; then
    echo "⚠️  Services already running. Stopping them first..."
    pkill -f "order_gate_server|ubscore_aeron_service" || true
    sleep 2
fi

# Setup infrastructure directly (don't call test 01 which has cleanup trap)
echo "📋 Setting up infrastructure..."

# Clean environment
echo "🧹 Cleaning environment..."
docker-compose down -v 2>/dev/null || true
docker rm -f tigerbeetle 2>/dev/null || true
rm -rf data/tigerbeetle/* logs/*
mkdir -p data/tigerbeetle logs

# Start Docker infrastructure
echo "🐳 Starting Docker infrastructure..."
docker-compose up -d --remove-orphans

# Wait for ScyllaDB
echo -n "⏳ Waiting for ScyllaDB (9042)..."
timeout=60
count=0
while [ $count -lt $timeout ]; do
    if nc -z localhost 9042 2>/dev/null; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# Apply ScyllaDB schema with retries
echo "📜 Applying ScyllaDB schema..."
docker cp schema/settlement_unified.cql scylla:/tmp/schema.cql
timeout=60
count=0
while [ $count -lt $timeout ]; do
    if docker exec scylla cqlsh -f /tmp/schema.cql 2>/dev/null; then
        echo -e " ${GREEN}✅ Settlement Schema Applied${NC}"
        # Apply Internal Transfer Schema
        docker cp schema/internal_transfer.cql scylla:/tmp/internal_transfer.cql
        if docker exec scylla cqlsh -f /tmp/internal_transfer.cql; then
             echo -e " ${GREEN}✅ Internal Transfer Schema Applied${NC}"
        fi
        break
    fi
    sleep 2
    let count=count+2
    echo -n "."
done

# Setup TigerBeetle
echo "🐯 Setting up TigerBeetle..."
docker run --privileged --rm -v $(pwd)/data/tigerbeetle:/data \
    ghcr.io/tigerbeetle/tigerbeetle:latest \
    format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle > /dev/null

docker run --privileged -d --name tigerbeetle -p 3000:3000 \
    -v $(pwd)/data/tigerbeetle:/data \
    ghcr.io/tigerbeetle/tigerbeetle:latest \
    start --addresses=0.0.0.0:3000 /data/0_0.tigerbeetle > /dev/null

echo -n "⏳ Waiting for TigerBeetle (3000)..."
timeout=10
count=0
while [ $count -lt $timeout ]; do
    if nc -z localhost 3000 2>/dev/null; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# Build services
echo "🔨 Building services..."
cargo build --bin ubscore_aeron_service --bin order_gate_server --features aeron --quiet

# Start UBSCore
echo "▶️  Starting UBSCore Service..."
rm -rf ~/ubscore_data
SEED_TEST_ACCOUNTS=0 ./target/debug/ubscore_aeron_service --features aeron > logs/ubscore_std.log 2>&1 &
UBS_PID=$!

DATE=$(date +%Y-%m-%d)
UBS_LOG="logs/ubscore.log.$DATE"

echo -n "⏳ Waiting for UBSCore..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if [ -f "$UBS_LOG" ] && grep -q "UBSCore Service ready" "$UBS_LOG"; then
        echo -e " ${GREEN}READY${NC} (PID: $UBS_PID)"
        break
    fi
    if ! kill -0 $UBS_PID 2>/dev/null; then
        echo -e " ${RED}UBSCore died!${NC}"
        tail -20 "$UBS_LOG" 2>/dev/null || cat logs/ubscore_std.log
        exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

if [ $count -ge $timeout ]; then
    echo -e " ${RED}TIMEOUT${NC}"
    exit 1
fi

# Start Gateway
echo "▶️  Starting Gateway Service..."
./target/debug/order_gate_server --features aeron > logs/gateway_std.log 2>&1 &
GW_PID=$!

GW_LOG="logs/gateway.log.$DATE"
echo -n "⏳ Waiting for Gateway..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if [ -f "$GW_LOG" ] && grep -q "Gateway starting" "$GW_LOG"; then
        echo -e " ${GREEN}READY${NC} (PID: $GW_PID)"
        break
    fi
    if ! kill -0 $GW_PID 2>/dev/null; then
        echo -e " ${RED}Gateway died!${NC}"
        cat logs/gateway_std.log
        exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

if [ $count -ge $timeout ]; then
    echo -e " ${RED}TIMEOUT${NC}"
    exit 1
fi

# Wait a bit more for Kafka connections to stabilize
echo "⏳ Stabilizing connections..."
sleep 3

echo ""
echo "📊 Service Status:"
echo "  ✅ UBSCore:  PID $UBS_PID (logs/ubscore.log.$DATE)"
echo "  ✅ Gateway:  PID $GW_PID (logs/gateway.log.$DATE)"
echo "  ✅ Endpoint: http://localhost:3001"
echo ""
echo -e "${GREEN}🎉 TEST 03 PASSED - Services Running${NC}"
echo ""
echo "💡 Services are running in background. To test:"
echo "   bash tests/04_http_api_test.sh"
echo ""
echo "💡 To stop services:"
echo "   pkill -f 'order_gate_server|ubscore_aeron_service'"
echo ""

exit 0
