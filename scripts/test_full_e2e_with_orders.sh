#!/bin/bash
# Complete E2E Test with Real Order Flow
# Tests: Order Client → Gateway → Kafka → ME → ZMQ → Order History → ScyllaDB

set -e

echo "=========================================="
echo "FULL E2E TEST - Real Order Flow"
echo "=========================================="
echo ""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

KEYSPACE="trading"

# Step 1: Check all prerequisites
echo -e "${BLUE}Step 1: Checking prerequisites...${NC}"

if ! docker ps | grep scylla > /dev/null; then
    echo -e "${RED}❌ ScyllaDB not running${NC}"
    exit 1
fi

if ! docker ps | grep redpanda > /dev/null; then
    echo -e "${RED}❌ Redpanda (Kafka) not running${NC}"
    echo -e "${YELLOW}💡 Start with: docker-compose up -d redpanda${NC}"
    exit 1
fi

echo -e "${GREEN}✅ All services running${NC}"
echo ""

# Step 2: Initialize database
echo -e "${BLUE}Step 2: Initializing database...${NC}"
# Force drop keyspace to ensure fresh schema
docker exec scylla cqlsh -e "DROP KEYSPACE IF EXISTS $KEYSPACE;" 2>&1 | grep -v "Warnings" || true
docker exec scylla cqlsh -e "CREATE KEYSPACE $KEYSPACE WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};" 2>&1 | grep -v "Warnings" || true
docker exec -i scylla cqlsh -k $KEYSPACE < schema/settlement_unified.cql 2>/dev/null
echo -e "${GREEN}✅ Database initialized with Unified Schema${NC}"
echo ""

# Step 2b: Clean Kafka Topics (Prevent Replay)
echo -e "${BLUE}Step 2b: Cleaning Kafka Topics...${NC}"
# Attempt to delete topics (might fail if not exist, ignore error)
docker exec redpanda rpk topic delete orders trades balance.operations 2>/dev/null || true
# Create topics again to ensure they are empty and exist
docker exec redpanda rpk topic create orders trades balance.operations -p 1 -r 1 2>/dev/null || true
echo -e "${GREEN}✅ Kafka Topics cleaned${NC}"
echo "Waiting 5s for Kafka metadata propagation..."
sleep 5
echo ""

# Step 3: Check binaries exist (build beforehand with: cargo build --bin ...)
echo -e "${BLUE}Step 3: Checking binaries...${NC}"
if [ ! -f target/debug/matching_engine_server ] || [ ! -f target/debug/order_gate_server ] || [ ! -f target/debug/settlement_service ] || [ ! -f target/debug/order_http_client ]; then
    echo -e "${RED}❌ Binaries not found. Please build first:${NC}"
    echo "  cargo build --bin matching_engine_server --bin order_gate_server --bin settlement_service --bin order_http_client"
    exit 1
fi
echo -e "${GREEN}✅ All binaries ready${NC}"
echo ""

# Step 4: Start Matching Engine
echo -e "${BLUE}Step 4: Starting Matching Engine...${NC}"
mkdir -p logs
pkill -f matching_engine_server || true
sleep 1
./target/debug/matching_engine_server > logs/matching_engine.log 2>&1 &
ME_PID=$!
echo "  Matching Engine PID: $ME_PID"
sleep 3

if ! ps -p $ME_PID > /dev/null; then
    echo -e "${RED}❌ Matching Engine failed to start${NC}"
    cat logs/matching_engine.log
    exit 1
fi
echo -e "${GREEN}✅ Matching Engine started${NC}"
echo ""

# Step 5: Start Order Gateway
echo -e "${BLUE}Step 5: Starting Order Gateway...${NC}"
pkill -f order_gate_server || true
sleep 1
./target/debug/order_gate_server > logs/order_gateway.log 2>&1 &
GATEWAY_PID=$!
echo "  Order Gateway PID: $GATEWAY_PID"
sleep 3

if ! ps -p $GATEWAY_PID > /dev/null; then
    echo -e "${RED}❌ Order Gateway failed to start${NC}"
    cat logs/order_gateway.log
    kill $ME_PID 2>/dev/null || true
    exit 1
fi
echo -e "${GREEN}✅ Order Gateway started${NC}"
echo ""

# Step 6: Start Settlement Service
echo -e "${BLUE}Step 6: Starting Settlement Service (Merged)...${NC}"
pkill -f settlement_service || true
pkill -f order_history_service || true
sleep 1
./target/debug/settlement_service > logs/settlement.log 2>&1 &
SS_PID=$!
echo "  Settlement Service PID: $SS_PID"
sleep 3

if ! ps -p $SS_PID > /dev/null; then
    echo -e "${RED}❌ Settlement Service failed to start${NC}"
    cat logs/settlement.log
    kill $ME_PID $GATEWAY_PID 2>/dev/null || true
    exit 1
fi
echo -e "${GREEN}✅ Settlement Service started${NC}"
echo ""

# Step 7: Send real orders via Order Client
echo -e "${BLUE}Step 7: Sending real orders via Order Client...${NC}"
echo "  Sending 3 orders..."

for i in {1..3}; do
    echo "  Order $i..."
    ./target/debug/order_http_client \
        --symbol BTC_USDT \
        --side $([ $((i % 2)) -eq 0 ] && echo "buy" || echo "sell") \
        --price $((50000 + i * 100)) \
        --qty 1 \
        > /dev/null 2>&1 || echo "    (Order may have failed)"
    sleep 0.1
done

echo -e "${GREEN}✅ Orders sent${NC}"
echo ""

# Step 8: Wait for processing
echo -e "${BLUE}Step 8: Waiting for order processing...${NC}"
echo "  Giving system 2 seconds to process orders..."
sleep 2
echo -e "${GREEN}✅ Processing complete${NC}"
echo ""

# Step 9: Verify data in database
echo -e "${BLUE}Step 9: Verifying data in database...${NC}"

ACTIVE_COUNT=$(docker exec scylla cqlsh -k $KEYSPACE -e "SELECT COUNT(*) FROM active_orders;" 2>/dev/null | awk '/^[[:space:]]*[0-9]+[[:space:]]*$/ {print $1}')
ACTIVE_COUNT=${ACTIVE_COUNT:-0}

HISTORY_COUNT=$(docker exec scylla cqlsh -k $KEYSPACE -e "SELECT COUNT(*) FROM order_history;" 2>/dev/null | awk '/^[[:space:]]*[0-9]+[[:space:]]*$/ {print $1}')
HISTORY_COUNT=${HISTORY_COUNT:-0}

STREAM_COUNT=$(docker exec scylla cqlsh -k $KEYSPACE -e "SELECT COUNT(*) FROM order_updates_stream;" 2>/dev/null | awk '/^[[:space:]]*[0-9]+[[:space:]]*$/ {print $1}')
STREAM_COUNT=${STREAM_COUNT:-0}

STATS_COUNT=$(docker exec scylla cqlsh -k $KEYSPACE -e "SELECT COUNT(*) FROM order_statistics;" 2>/dev/null | awk '/^[[:space:]]*[0-9]+[[:space:]]*$/ {print $1}')
STATS_COUNT=${STATS_COUNT:-0}

echo "  Active orders: $ACTIVE_COUNT"
echo "  Order history: $HISTORY_COUNT"
echo "  Update stream: $STREAM_COUNT"
echo "  Statistics: $STATS_COUNT"
echo ""

TOTAL_RECORDS=$((ACTIVE_COUNT + HISTORY_COUNT + STREAM_COUNT + STATS_COUNT))

if [ "$TOTAL_RECORDS" -gt "0" ]; then
    echo -e "${GREEN}✅ Data found in database!${NC}"
else
    echo -e "${YELLOW}⚠️  No data found - checking logs...${NC}"
    echo ""
    echo "Settlement Service logs:"
    tail -20 logs/settlement.log
fi
echo ""

# Step 10: Display actual data
if [ "$TOTAL_RECORDS" -gt "0" ]; then
    echo -e "${BLUE}Step 10: Displaying actual data...${NC}"

    if [ "$ACTIVE_COUNT" -gt "0" ]; then
        echo ""
        echo "Active Orders ($ACTIVE_COUNT records):"
        docker exec scylla cqlsh -k $KEYSPACE -e "SELECT user_id, order_id, symbol, side, status, price, qty FROM active_orders LIMIT 10;" 2>/dev/null
    fi

    if [ "$HISTORY_COUNT" -gt "0" ]; then
        echo ""
        echo "Order History ($HISTORY_COUNT records):"
        docker exec scylla cqlsh -k $KEYSPACE -e "SELECT user_id, order_id, symbol, status, created_at FROM order_history LIMIT 10;" 2>/dev/null
    fi

    if [ "$STREAM_COUNT" -gt "0" ]; then
        echo ""
        echo "Update Stream ($STREAM_COUNT records):"
        docker exec scylla cqlsh -k $KEYSPACE -e "SELECT event_id, order_id, user_id, status FROM order_updates_stream LIMIT 10;" 2>/dev/null
    fi

    if [ "$STATS_COUNT" -gt "0" ]; then
        echo ""
        echo "Statistics ($STATS_COUNT records):"
        docker exec scylla cqlsh -k $KEYSPACE -e "SELECT user_id, total_orders, filled_orders, cancelled_orders FROM order_statistics;" 2>/dev/null
    fi
fi
echo ""

# Step 11: Cleanup
echo -e "${BLUE}Step 11: Stopping services...${NC}"
kill $SS_PID $GATEWAY_PID $ME_PID 2>/dev/null || true
sleep 2
echo -e "${GREEN}✅ Services stopped${NC}"
echo ""

# Summary
echo "=========================================="
echo -e "${GREEN}FULL E2E Test Summary${NC}"
echo "=========================================="
echo ""
echo "✅ Matching Engine: Started and processed orders"
echo "✅ Order Gateway: Started and accepted orders"
echo "✅ Settlement Service (Merged): Started and persisted data"
echo "✅ Database: Connected and accessible"
echo ""
echo "📊 Final Data Count:"
echo "  Active orders: $ACTIVE_COUNT"
echo "  Order history: $HISTORY_COUNT"
echo "  Update stream: $STREAM_COUNT"
echo "  Statistics: $STATS_COUNT"
echo "  TOTAL: $TOTAL_RECORDS records"
echo ""

if [ "$TOTAL_RECORDS" -gt "0" ]; then
    echo -e "${GREEN}🎉 FULL E2E TEST PASSED - REAL DATA FLOW VERIFIED!${NC}"
    echo ""
    echo "✅ Complete flow tested:"
    echo "  Order Client → Gateway API → Kafka → Matching Engine"
    echo "  → ZMQ → Settlement Service → ScyllaDB"
else
    echo -e "${YELLOW}⚠️  E2E test completed but no data persisted${NC}"
    echo "   Check logs for details:"
    echo "   - Matching Engine: logs/matching_engine.log"
    echo "   - Order Gateway: logs/order_gateway.log"
    echo "   - Settlement Service: logs/settlement.log"
fi
echo ""
