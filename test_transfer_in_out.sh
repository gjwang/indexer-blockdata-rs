#!/bin/bash

set -e  # Exit on error

echo "=========================================="
echo "Deposit/Withdrawal System - Quick Test"
echo "=========================================="
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check Redpanda
echo -e "${YELLOW}1. Checking Redpanda...${NC}"
if docker ps | grep -q redpanda; then
    echo -e "${GREEN}✅ Redpanda is running${NC}"
else
    echo -e "${RED}❌ Redpanda is NOT running!${NC}"
    echo "Start it with:"
    echo "  docker run -d --name redpanda -p 9092:9092 -p 9093:9093 \\"
    echo "    docker.redpanda.com/redpandadata/redpanda:latest \\"
    echo "    redpanda start --smp 1 --memory 1G"
    exit 1
fi
echo ""

# Check if binaries are built
echo -e "${YELLOW}2. Checking binaries...${NC}"
if [ ! -f "target/debug/transfer_server" ]; then
    echo -e "${YELLOW}Building binaries...${NC}"
    cargo build \
        --bin transfer_server \
        --bin matching_engine_server \
        --bin balance_test_client
fi
echo -e "${GREEN}✅ Binaries ready${NC}"
echo ""

# Check if gateway is running
echo -e "${YELLOW}3. Checking Transfer Server...${NC}"
if curl -s http://localhost:8083/health > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Transfer Server is running on :8083${NC}"
else
    echo -e "${RED}❌ Transfer Server is NOT running!${NC}"
    echo "Start it in another terminal with:"
    echo "  cargo run --bin transfer_server"
    exit 1
fi
echo ""

# Check if matching engine is running
echo -e "${YELLOW}4. Checking Matching Engine...${NC}"
# We assume it's running or user will start it.
# Ideally we check for the process or a health endpoint if it had one.
echo -e "${YELLOW}   (Ensure matching_engine_server is running in Terminal 2)${NC}"
echo ""

# Run test client
echo -e "${YELLOW}5. Running Test Client...${NC}"
echo "=========================================="
cargo run --bin balance_test_client
echo "=========================================="
echo ""

echo -e "${GREEN}✅ All tests completed!${NC}"
echo ""
echo "Check the matching_engine_server terminal for detailed logs."
echo ""
