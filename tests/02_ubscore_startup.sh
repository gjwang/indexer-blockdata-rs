#!/bin/bash
set -e

# Test 02: UBSCore Service Startup
# Verifies UBSCore starts correctly and initializes TigerBeetle accounts

echo "🧪 TEST 02: UBSCore Service Startup"
echo "======================================"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Note: We setup infrastructure inline WITHOUT cleanup trap
# Test 01 has a cleanup trap that kills Docker - we can't use it
echo "📋 Setting up infrastructure (inline)..."

# Clean and start Docker
docker-compose down -v 2>/dev/null || true
docker-compose up -d

# Wait for ScyllaDB port to open
echo -n "⏳ Waiting for ScyllaDB port (9042)..."
count=0
while [ $count -lt 30 ]; do
    if nc -z localhost 9042 2>/dev/null; then
        echo " PORT OPEN"
        break
    fi
    sleep 2
    let count=count+1
done

# ScyllaDB needs extra time after port opens to be ready for queries
echo "⏳ Waiting for ScyllaDB to be fully ready (15s)..."
sleep 15

# Apply schema with retries
echo "📜 Applying ScyllaDB schema (with retries)..."
retries=5
for i in $(seq 1 $retries); do
    if docker exec scylla cqlsh -e "$(cat schema/settlement_unified.cql)" 2>/dev/null; then
        echo " ✅ Schema applied"
        break
    fi
    echo " Retry $i/$retries..."
    sleep 5
done

# Wait for Redpanda port to open
echo -n "⏳ Waiting for Redpanda port (9092)..."
count=0
while [ $count -lt 30 ]; do
    if nc -z localhost 9092 2>/dev/null; then
        echo " PORT OPEN"
        break
    fi
    sleep 2
    let count=count+1
done

# Redpanda also needs time after port opens for broker initialization
echo "⏳ Waiting for Redpanda/Kafka to be fully ready (20s)..."
sleep 20

# Start TigerBeetle (using Docker like test 01)
echo "🐯 Starting TigerBeetle..."
docker rm -f tigerbeetle 2>/dev/null || true
mkdir -p data/tigerbeetle
rm -f data/tigerbeetle/0_0.tig*

docker run --privileged --rm -v $(pwd)/data/tigerbeetle:/data \
    ghcr.io/tigerbeetle/tigerbeetle:latest \
    format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle > /dev/null

docker run --privileged -d --name tigerbeetle -p 3000:3000 \
    -v $(pwd)/data/tigerbeetle:/data \
    ghcr.io/tigerbeetle/tigerbeetle:latest \
    start --addresses=0.0.0.0:3000 /data/0_0.tigerbeetle > /dev/null

sleep 3

echo -e " ${GREEN}✅ Infrastructure ready${NC}"


# Cleanup function
cleanup() {
    echo "🧹 Cleanup..."
    pkill -f ubscore_aeron_service || true
}
trap cleanup EXIT

# Build UBSCore
echo "🔨 Building UBSCore service..."
cargo build --bin ubscore_aeron_service --features aeron --quiet

# Wait for Kafka to be FULLY ready for external connections on port 9093
# This is critical - port open doesn't mean Kafka broker is ready
echo -n "⏳ Actively waiting for Kafka broker on port 9093..."
timeout=60
count=0
kafka_ready=false

while [ $count -lt $timeout ]; do
    # Try to connect to Kafka using nc (netcat)
    if echo "test" | nc -w 1 localhost 9093 >/dev/null 2>&1; then
        # Port accepts connection, wait a bit more for broker to be fully ready
        sleep 5
        kafka_ready=true
        echo -e " ${GREEN}READY${NC} (after ${count}s)"
        break
    fi
    sleep 2
    let count=count+2
    if [ $((count % 10)) -eq 0 ]; then
        echo -n "."
    fi
done

if [ "$kafka_ready" = "false" ]; then
    echo -e " ${RED}TIMEOUT after ${timeout}s${NC}"
    echo "Kafka on port 9093 not responding - continuing anyway, UBSCore will retry"
fi


# Start UBSCore
echo "▶️  Starting UBSCore Service..."
rm -rf ~/ubscore_data logs/*
SEED_TEST_ACCOUNTS=0 ./target/debug/ubscore_aeron_service --features aeron > logs/ubscore_std.log 2>&1 &
UBS_PID=$!

# Wait for UBSCore to  be ready
DATE=$(date +%Y-%m-%d)
LOG_FILE="logs/ubscore.log.$DATE"

echo -n "⏳ Waiting for UBSCore..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if [ -f "$LOG_FILE" ] && grep -q "UBSCore Service ready" "$LOG_FILE"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    if ! kill -0 $UBS_PID 2>/dev/null; then
        echo -e " ${RED}UBSCore died!${NC}"
        cat logs/ubscore_std.log
        exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

if [ $count -ge $timeout ]; then
    echo-e " ${RED}TIMEOUT${NC}"
    tail -n 20 "$LOG_FILE" 2>/dev/null || cat logs/ubscore_std.log
    exit 1
fi

# Verify TigerBeetle initialization
echo "🐯 Verifying TigerBeetle initialization..."
if grep -q "System accounts initialized" "$LOG_FILE"; then
    echo -e " ${GREEN}✅ TigerBeetle accounts initialized${NC}"
else
    echo -e " ${RED}❌ TigerBeetle initialization failed${NC}"
    tail -n 20 "$LOG_FILE"
    exit 1
fi

# Verify event loop started
if grep -q "Starting event loop" "$LOG_FILE"; then
    echo -e " ${GREEN}✅ Event loop started${NC}"
else
    echo -e " ${RED}❌ Event loop not started${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}🎉 TEST 02 PASSED - UBSCore Running${NC}"
exit 0
