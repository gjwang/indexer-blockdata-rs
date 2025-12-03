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
if [ ! -f "target/release/deposit_withdraw_gateway" ]; then
    echo -e "${YELLOW}Building binaries...${NC}"
    cargo build --release \
        --bin deposit_withdraw_gateway \
        --bin balance_processor \
        --bin balance_test_client
fi
echo -e "${GREEN}✅ Binaries ready${NC}"
echo ""

# Check if gateway is running
echo -e "${YELLOW}3. Checking Gateway...${NC}"
if curl -s http://localhost:8082/health > /dev/null 2>&1; then
    echo -e "${GREEN}✅ Gateway is running on :8082${NC}"
else
    echo -e "${RED}❌ Gateway is NOT running!${NC}"
    echo "Start it in another terminal with:"
    echo "  cargo run --bin deposit_withdraw_gateway --release"
    exit 1
fi
echo ""

# Check if balance processor is running
echo -e "${YELLOW}4. Checking Balance Processor...${NC}"
echo -e "${YELLOW}   (Assuming it's running - check Terminal 2)${NC}"
echo ""

# Run test client
echo -e "${YELLOW}5. Running Test Client...${NC}"
echo "=========================================="
cargo run --bin balance_test_client --release
echo "=========================================="
echo ""

echo -e "${GREEN}✅ All tests completed!${NC}"
echo ""
echo "Check the balance_processor terminal for detailed logs."
echo ""
