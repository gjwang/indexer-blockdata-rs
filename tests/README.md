# Test Suite

Modern test suite for the HFT Exchange System with TigerBeetle integration.

## Test Structure (Simple → Comprehensive)

### **01_infrastructure.sh** - Infrastructure & Database
- Verifies Docker services start correctly (ScyllaDB, Redpanda, StarRocks, TigerBeetle)
- Tests database schema application
- Validates Kafka topics creation
- **Duration**: ~30 seconds
- **Dependencies**: Docker, docker-compose

### **02_ubscore_startup.sh** - UBSCore Service Initialization
- Tests UBSCore service starts correctly with TigerBeetle
- Validates TigerBeetle account initialization
- Checks logging system works
- **Duration**: ~10 seconds
- **Dependencies**: Test 01

### **03_deposit_withdrawal.sh** - Balance Operations via Gateway
- Tests HTTP deposit API (Gateway → Kafka → UBSCore → TigerBeetle)
- Validates balance queries from TigerBeetle
- Tests withdrawal flow
- **Duration**: ~15 seconds
- **Dependencies**: Test 02

### **04_order_placement.sh** - Order Flow (No Matching)
- Tests order submission via Gateway
- Validates UBSCore risk checks
- Verifies orders are published to Kafka for ME
- **Duration**: ~20 seconds
- **Dependencies**: Test 03

### **05_gateway_e2e.sh** ✅ - Gateway E2E (Existing)
- Complete deposit + balance query flow
- HTTP API → Gateway → Kafka → UBSCore → TigerBeetle
- Balance verification via Gateway query
- **Duration**: ~2 minutes (includes full build)
- **Status**: **PASSING** ✅

### **06_matching_engine.sh** - Matching Engine Integration
- Starts Matching Engine with symbol configuration
- Tests order matching logic
- Validates trade generation
- **Duration**: ~30 seconds
- **Dependencies**: Test 04

### **07_settlement.sh** - Settlement Service
- Tests Settlement consumes ME outputs from Kafka
- Validates trades written to ScyllaDB
- Checks balance events processing
- **Duration**: ~25 seconds
- **Dependencies**: Test 06

### **08_full_trading_flow.sh** - Complete Trading E2E
- All services: Gateway, UBSCore, Matching Engine, Settlement
- Complete flow: Deposit → Order → Match → Settlement → Query
- Validates:
  - User deposits funds
  - Places buy/sell orders
  - Orders match and generate trades
  - Trades settle correctly
  - Balances update in TigerBeetle
  - Order history in ScyllaDB
- **Duration**: ~3 minutes
- **Dependencies**: All previous tests

## Running Tests

### Individual Test
```bash
bash tests/03_deposit_withdrawal.sh
```

### Full Suite (Sequential)
```bash
bash tests/run_all_tests.sh
```

### Quick Smoke Test
```bash
bash tests/05_gateway_e2e.sh
```

## Test Environment

All tests use:
- **UBSCore**: `ubscore_aeron_service` (Aeron IPC mode)
- **Gateway**: `order_gate_server` (HTTP + Kafka)
- **Matching Engine**: `matching_engine_server` (Kafka input/output)
- **Settlement**: `settlement_service` (Kafka consumer)
- **Storage**: TigerBeetle (balances), ScyllaDB (trades/orders/history)

## Legacy Tests

Old file-based command tests (pre-Kafka) are archived in `tests/legacy/`:
- `01_basic_deposit.sh` - Old deposit test using file triggers
- `02_order_lifecycle.sh` - Old lock/unlock test
- `03_trade_settlement.sh` - Old settlement test
- `04_full_simulation.sh` - Old simulation test
- `common.sh` - Legacy test helpers

These are **deprecated** and incompatible with the current Kafka/HTTP architecture.

## Test Data Cleanup

```bash
# Clean all test data
docker-compose down -v
docker rm -f tigerbeetle
rm -rf data/tigerbeetle/* logs/* ~/ubscore_data
```

## CI/CD Integration

Recommended test order for CI:
1. `01_infrastructure.sh` - Fail fast if infrastructure broken
2. `05_gateway_e2e.sh` - Quickest full E2E validation
3. `08_full_trading_flow.sh` - Comprehensive validation

Parallel execution not recommended due to port conflicts.
