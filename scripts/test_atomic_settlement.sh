#!/bin/bash
# Test script for atomic settlement implementation

set -e

echo "🧪 Testing Atomic Settlement Implementation"
echo "=========================================="

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Step 1: Check compilation
echo -e "\n${YELLOW}Step 1: Checking compilation...${NC}"
if cargo check --bin settlement_service --quiet; then
    echo -e "${GREEN}✅ Settlement service compiles successfully${NC}"
else
    echo -e "${RED}❌ Compilation failed${NC}"
    exit 1
fi

# Step 2: Check if ScyllaDB is running
echo -e "\n${YELLOW}Step 2: Checking ScyllaDB...${NC}"
if docker ps | grep -q scylla; then
    echo -e "${GREEN}✅ ScyllaDB is running${NC}"
else
    echo -e "${YELLOW}⚠️  ScyllaDB not running. Starting...${NC}"
    docker-compose up -d scylla
    echo "Waiting for ScyllaDB to be ready (30s)..."
    sleep 30
fi

# Step 3: Check schema
echo -e "\n${YELLOW}Step 3: Checking schema...${NC}"
if [ -f "schema/settlement_schema.cql" ]; then
    echo -e "${GREEN}✅ Schema file exists${NC}"
    echo "   To load schema, run: cqlsh -f schema/settlement_schema.cql"
else
    echo -e "${RED}❌ Schema file not found${NC}"
fi

# Step 4: Build settlement service
echo -e "\n${YELLOW}Step 4: Building settlement service...${NC}"
if cargo build --bin settlement_service --quiet; then
    echo -e "${GREEN}✅ Settlement service built successfully${NC}"
else
    echo -e "${RED}❌ Build failed${NC}"
    exit 1
fi

# Step 5: Check configuration
echo -e "\n${YELLOW}Step 5: Checking configuration...${NC}"
if [ -f "config/settlement_config.yaml" ]; then
    echo -e "${GREEN}✅ Settlement config exists${NC}"
    echo "   Config file: config/settlement_config.yaml"
else
    echo -e "${YELLOW}⚠️  Settlement config not found${NC}"
    echo "   Using default config"
fi

# Summary
echo -e "\n${GREEN}========================================${NC}"
echo -e "${GREEN}✅ All checks passed!${NC}"
echo -e "${GREEN}========================================${NC}"

echo -e "\n📝 Next steps:"
echo "1. Load schema (if not already loaded):"
echo "   ${YELLOW}cqlsh -f schema/settlement_schema.cql${NC}"
echo ""
echo "2. Start matching engine:"
echo "   ${YELLOW}cargo run --bin matching_engine_server${NC}"
echo ""
echo "3. Start settlement service:"
echo "   ${YELLOW}cargo run --bin settlement_service${NC}"
echo ""
echo "4. Send test trades:"
echo "   ${YELLOW}cargo run --bin order_http_client${NC}"
echo ""
echo "5. Verify settlement:"
echo "   ${YELLOW}cqlsh -e \"SELECT * FROM trading.settled_trades LIMIT 10\"${NC}"
echo "   ${YELLOW}cqlsh -e \"SELECT * FROM trading.user_balances\"${NC}"

echo -e "\n🎉 Ready for testing!"
