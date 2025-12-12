# Internal Transfer Fix Plan

**Status:** Completed
**Objective:** Operationalize the Internal Transfer Background Service and fix critical recovery logic flaws to ensure funds are not locked indefinitely.

## 1. Overview
The implementation of Funding->Spot transfers creates "Pending" (Locked) transfers in TigerBeetle but checks for them in a disconnected background service. We have now connected the service and fixed the logic.

## 2. Implementation Steps

### Step 1: Fix Recovery Logic (`src/api/internal_transfer_settlement.rs`)
**Status:** ✅ DONE
**Changes:**
1.  Imports `TransferFlags`.
2.  Checks `flags.contains(TransferFlags::PENDING)`.
3.  Executes `POST_PENDING_TRANSFER` if found.
4.  Logs actions clearly.

### Step 2: Activate Background Service (`src/bin/internal_transfer_settlement.rs`)
**Status:** ✅ DONE
**Changes:**
1.  Now imports logic from `fetcher::api` (DRY).
2.  Spawns `settlement.run_scanner()` in a Tokio background task.
3.  Runs consumer in main thread.

### Step 3: Verify
**Status:** ✅ DONE
**Action:**
*   Created `tests/14_rescue_transfer_e2e.sh` to specifically test recovery.
*   Updated `tests/13_spot_funding_e2e.sh` to include settlement service startup.
*   Verified successful end-to-end settlement.

### Step 4: Fix Process Confirmation (Idempotency Bug)
**Status:** ✅ DONE
**Changes:**
1.  Updated `process_confirmation` in `src/api/internal_transfer_settlement.rs`.
2.  Added `ensure_account` calls to prevent `CreditAccountNotFound` errors.
3.  Improved error handling to allow idempotent retries ("Exists" is success).

## 3. Verification Plan

### Test Case: "The Rescue"
**Status:** ✅ PASS
1.  **Setup:**
    *   Stop the `internal_transfer_settlement` service.
    *   Send a `Funding -> Spot` transfer via API.
    *   Verify API returns "Success" (but data is status="Pending").
    *   Verify DB status is "Pending".
2.  **Action:**
    *   Start `internal_transfer_settlement` service.
3.  **Expectation:**
    *   Scanner picks up the "Pending" record.
    *   Scanner sees it is Pending in TB.
    *   Scanner posts the transfer.
    *   Scanner updates DB to "Success".
