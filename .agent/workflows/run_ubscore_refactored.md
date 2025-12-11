---
description: Run the refactored UBSCore Service with TigerBeetle Worker
---

This workflow starts the redpanda and tigerbeetle containers, then runs the `ubscore_service` which now includes the embedded TigerBeetle Worker consuming BalanceEvents.

1. Start Infrastructure
```bash
docker start redpanda tigerbeetle
```

2. Run UBSCore Service
```bash
# This will compile and run the service.
# The service will:
# - Connect to Redpanda
# - Register default symbols (BTC/USDT)
# - Connect to TigerBeetle (embedded worker)
# - Serialize events to internal channel
# - Sync events to TigerBeetle
cargo run --bin ubscore_service
```

3. Verify Logs
   Look for:
   - `[LOCK] user=...` (Fund locking)
   - `TigerBeetle Shadow Ledger sync started`
   - `Shadow Sync OK` (TigerBeetle success)
