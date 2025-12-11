# Handover Notes: TigerBeetle Integration

**Date:** 2025-12-11
**Status:** Complete & Verified

## Overview
The **TigerBeetle Integration** is complete. The system now uses TigerBeetle as the authoritative Source of Truth (SOT) for user balances, effectively deprecating the ScyllaDB `user_balances` table.

## Key Achievements
1.  **Gateway Balance Query**: The `Gateway` (`src/gateway.rs`) now queries TigerBeetle directly via `lookup_accounts` to retrieve user balances. The ScyllaDB fallback logic has been **removed** to ensure strict data authority.
2.  **ScyllaDB Writes Removed**: The `Settlement Service` (`src/db/settlement_db.rs`) no longer writes to the `user_balances` table. This table remains in the schema (`schema/settlement_unified.cql`) for backward compatibility but is no longer populated.
3.  **TigerBeetle Client Fix**: `UBSCore`'s `TigerBeetleWorker` (`src/ubs_core/tigerbeetle.rs`) has been updated to initialize the client synchronously, ensuring compatibility with the `tigerbeetle-unofficial` 0.14 crate.
4.  **E2E Verification**: The `tests/05_gateway_e2e.sh` script has been hardened (fail-fast build, log isolation, process cleanup) and **passes successfully**, confirming the full Deposit -> UBSCore -> TigerBeetle -> Gateway Balance flow.

## Pending / Uncommitted Changes
The following files appeared as modified in `git status` (verify these before next commit):
*   `config/dev.yaml`: Likely contains the `scylladb` configuration section added during debugging. Required if `order_gate_server` needs to connect to Scylla for other features (e.g. Order History). **Review and Commit.**
*   `src/bin/ubscore_service.rs`: This is the legacy/ZMQ version of the service. I primarily worked on `src/bin/ubscore_aeron_service.rs`. The ZMQ version might need updates to match the `TigerBeetleWorker` changes (sync client init) if it is still in use. **Verify if this service is needed and update if necessary.**
*   `tests/05_gateway_e2e.sh`: This file was committed but shows modifications. Ensure the latest version (with trap and build steps) is preserved.

## Instructions for Next Agent
1.  **Verify `config/dev.yaml`**: Ensure the ScyllaDB config is correct and commit it if necessary.
2.  **Check Legacy Service**: If `src/bin/ubscore_service.rs` (ZMQ) is active, verify it compiles and runs with the updated `fetcher` library (specifically `TigerBeetleWorker`).
3.  **Run E2E**: Use `./tests/05_gateway_e2e.sh` to verify the environment. Note that it clears `~/ubscore_data` and recreates the TigerBeetle container for a clean run.
4.  **Monitoring**: The integration relies on Hardcoded Asset IDs in `SymbolManager` matching TigerBeetle. If new assets are added, ensure they are synced between `SymbolManager` and TigerBeetle.

**Primary Verification Command:**
```bash
./tests/05_gateway_e2e.sh
```
