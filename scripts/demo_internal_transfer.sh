#!/bin/bash
# Demo script for Internal Transfer feature

set -e

echo "========================================="
echo "  Internal Transfer Demo"
echo "========================================="
echo ""

# Colors
GREEN='\033[0.32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}Step 1: Build the project${NC}"
cargo build --lib
echo -e "${GREEN}✓ Build successful${NC}"
echo ""

echo -e "${BLUE}Step 2: Run unit tests${NC}"
cargo test --lib internal_transfer_types -- --test-threads=1
echo -e "${GREEN}✓ Unit tests passed${NC}"
echo ""

echo -e "${BLUE}Step 3: Run validation tests${NC}"
cargo test --lib internal_transfer_validator -- --test-threads=1
echo -e "${GREEN}✓ Validation tests passed${NC}"
echo ""

echo -e "${BLUE}Step 4: Run TigerBeetle mock tests${NC}"
cargo test --lib tigerbeetle_mock -- --test-threads=1
echo -e "${GREEN}✓ Mock tests passed${NC}"
echo ""

echo -e "${BLUE}Step 5: Summary${NC}"
echo "========================================="
echo "All tests passed!"
echo ""
echo "Features verified:"
echo "  ✓ Data structures and serialization"
echo "  ✓ Request validation (asset, amount, precision)"
echo "  ✓ TigerBeetle operations (PENDING/POST/VOID)"
echo "  ✓ Balance tracking"
echo ""
echo "Ready for:"
echo "  - Database integration"
echo "  - API endpoint creation"
echo "  - Full E2E testing"
echo "========================================="
