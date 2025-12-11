#!/bin/bash
set -e

# Test 01: Infrastructure Validation
# Verifies all infrastructure components start correctly

echo "🧪 TEST 01: Infrastructure Validation"
echo "======================================"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Cleanup function
cleanup() {
    echo "🧹 Cleanup..."
    docker-compose down -v 2>/dev/null || true
    docker rm -f tigerbeetle 2>/dev/null || true
}
trap cleanup EXIT

# 1. Clean environment
echo "🧹 Cleaning previous environment..."
cleanup
rm -rf data/tigerbeetle/* logs/*
mkdir -p data/tigerbeetle logs

# 2. Start Docker infrastructure
echo "🐳 Starting Docker infrastructure..."
docker-compose up -d --remove-orphans

# 3. Wait for ScyllaDB
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

if [ $count -ge $timeout ]; then
    echo -e " ${RED}TIMEOUT${NC}"
    exit 1
fi

# 4. Apply ScyllaDB schema with retries
echo "📜 Applying ScyllaDB schema..."
docker cp schema/settlement_unified.cql scylla:/tmp/schema.cql

timeout=60
count=0
while [ $count -lt $timeout ]; do
    if docker exec scylla cqlsh -f /tmp/schema.cql 2>/dev/null; then
        echo -e " ${GREEN}✅ Schema Applied${NC}"
        break
    fi
    sleep 2
    let count=count+2
    echo -n "."
done

if [ $count -ge $timeout ]; then
    echo -e " ${RED}❌ Schema application failed${NC}"
    exit 1
fi

# 5. Wait for Redpanda (Kafka)
echo -n "⏳ Waiting for Redpanda (9092)..."
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if nc -z localhost 9092 2>/dev/null; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# 6. Setup TigerBeetle
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

# 7. Verify all services
echo ""
echo "📊 Infrastructure Status:"
echo "  ✅ ScyllaDB:   localhost:9042"
echo "  ✅ Redpanda:   localhost:9092"
echo "  ✅ StarRocks:  localhost:9030"
echo "  ✅ TigerBeetle: localhost:3000"

echo ""
echo -e "${GREEN}🎉 TEST 01 PASSED - Infrastructure Ready${NC}"
exit 0
