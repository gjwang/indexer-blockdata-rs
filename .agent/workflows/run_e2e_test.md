---
description: Run the full end-to-end test for the trading system
---

This workflow runs the comprehensive E2E test script which verifies the entire trading pipeline, including:
1. Starting all services (Settlement, Matching Engine, Transfer, Gateway)
2. Initializing user balances via Transfer Server
3. Sending concurrent orders via Order Client
4. Verifying trade settlement and balance updates in ScyllaDB

# Steps

1. Ensure all dependencies are built (optional, script will build debug binaries)
```bash
cargo build
```

2. Run the E2E test script
// turbo
```bash
./test_full_e2e.sh
```

3. Check the output for "✅ E2E Test Complete!" and verify statistics.
