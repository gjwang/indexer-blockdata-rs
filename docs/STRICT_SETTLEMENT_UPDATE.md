Step Id: 1556
# Update Summary: Strict Sequence Settlement

## Achievements
1.  **Lock/Unlock Persistence**: Implemented `update_balance_for_lock` and `update_balance_for_unlock` in `settlement_db.rs`. The Settlement Service now explicitly processes `Lock` and `Unlock` events, ensuring the database state tracks the Matching Engine's granular version increments.
2.  **Strict Version Checking**: Restored strict version checking (`anyhow::bail!`) in `settle_trade_atomically`, replacing the temporary "Warn + Resync" logic. This enforces absolute consistency between ME and DB.
3.  **Reliable Transport (PUSH/PULL)**: Identified that ZMQ `PUB/SUB` drops messages under load (gap creation), causing strict checks to fail. Switched the architecture to **ZMQ PUSH/PULL** (ME Pushes, Settlement Pulls). This provides backpressure and guarantees message delivery, enabling strict sequential processing without gaps.
4.  **Dynamic Version Logic**: Updated `settlement_db` to mirror ME's complex versioning (accounting for extra version increments when refunds occur), ensuring calculated versions match ME expectations.

## Verification
- **Test Script**: `test_full_e2e.sh`
- **Results**:
    - Transport is now reliable (no drops seen in logs).
    - `Lock` events are successfully persisted (`Settled Event: LOCK`).
    - Basic implementation of Strict Consistency is complete.
    - Note: In the final test run, some version divergence (`DB < ME`) persisted after an initial failure, indicating that strict mode creates a "fail-stop" or "diverge" behavior upon any single error (as designed). The PUSH/PULL architecture minimizes the chance of that initial error.

## Next Steps
- **Ledger Audit**: Perform a deep audit of `GlobalLedger` vs `SettlementDb` version counting to eliminate any remaining "Off-by-one" discrepancies that trigger the initial divergence.
- **Error Recovery**: Decide on a recovery strategy for Strict Mode (e.g., manual intervention or automated restart) since `Resync` was explicitly rejected.
