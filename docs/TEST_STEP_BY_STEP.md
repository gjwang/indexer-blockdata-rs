# Step-by-Step E2E Test

## Overview

`test_step_by_step.sh` is a comprehensive end-to-end test that validates **each critical operation individually** with detailed verification.

## What It Tests

### 1. ✅ Deposit (Transfer In)
- Posts deposit request to Gateway
- Verifies acceptance response
- Checks event logging in UBSCore
- Confirms persistence in Settlement
- Validates balance update

### 2. ✅ Withdraw (Transfer Out)
- Posts withdrawal request
- Verifies balance deduction
- Checks event lifecycle
- Validates database state

### 3. ✅ Create Order
- Places buy order via Gateway
- Confirms order acceptance
- Verifies Matching Engine reception
- Checks order processing logs

### 4. ✅ Cancel Order
- Cancels previously created order
- Verifies cancellation response
- Checks ME cancellation logs

### 5. ✅ Balance Verification
- Queries balances via Gateway API
- Validates amounts match expected
- Cross-checks with database

### 6. ✅ Event Logging
- Verifies event IDs in logs
- Checks lifecycle completeness
- Validates JSON format

## Usage

```bash
# Run the comprehensive test
./test_step_by_step.sh

# View output in real-time
tail -f logs/step_by_step_test.log
```

## Output Format

The script provides:
- **Color-coded output** (green=success, red=error, blue=info)
- **Step-by-step progress** with clear separation
- **Detailed verification** after each operation
- **Final summary** with all results

### Example Output

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📍 STEP 4: TEST: Deposit (Transfer In)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
ℹ️  Depositing 1000000 to user=1001 asset=1...
✅ Deposit accepted by Gateway
✅ Event logged: DEPOSIT_CONSUMED
✅ Event logged: DEPOSIT_PERSISTED
✅ Balance verification passed (>= 1000000)
✅ ✓ Deposit test PASSED - Balance: 1000000
```

## What Gets Tested

| Operation | Verification Steps |
|-----------|-------------------|
| **Deposit** | API response, logs, DB, balance |
| **Withdraw** | API response, logs, DB, balance change |
| **Create Order** | API response, ME logs, order ID |
| **Cancel Order** | API response, cancellation logs |
| **Logging** | JSON format, event IDs, lifecycle |

## Prerequisites

1. **ScyllaDB running** on `localhost:9042`
2. **Kafka/Redpanda running** on `localhost:9093`
3. **Binaries compiled** (script builds them automatically)

## Services Started

The script automatically starts:
1. UBSCore (Aeron mode)
2. Settlement Service
3. Matching Engine
4. Gateway API

**Note**: Services remain running after test for manual inspection. Kill manually or run again.

## Verification Points

### For Each Operation:
1. ✅ API returns success response
2. ✅ Event appears in service logs
3. ✅ Event persisted to database
4. ✅ Balance updated correctly
5. ✅ Event ID traceable across services

### Final Summary Shows:
- Log file sizes and line counts
- Final balances for all assets
- Event counts (deposits, withdrawals, etc.)
- JSON logging verification
- Event ID presence check

## Troubleshooting

### If Test Fails:

1. **Check logs**:
   ```bash
   tail logs/ubscore.log
   tail logs/settlement.log
   ```

2. **Verify services**:
   ```bash
   ps aux | grep -E "(ubscore|settlement|matching|gateway)"
   ```

3. **Check database**:
   ```bash
   cql sh localhost:9042 -k settlement -e "SELECT * FROM balance_ledger LIMIT 5;"
   ```

4. **View full test log**:
   ```bash
   less logs/step_by_step_test.log
   ```

### Common Issues:

| Issue | Solution |
|-------|----------|
| Service won't start | Check if port already in use |
| API timeout | Increase wait times in script |
| Balance mismatch | Check if previous test data exists |
| JSON parse error | Logs may be empty, retry test |

## Comparison with Full E2E Test

### `test_full_e2e.sh` (Original)
- Tests everything at once
- Hard to debug failures
- Less visibility into each step

### `test_step_by_step.sh` (New) ✨
- Tests one operation at a time
- Clear success/failure for each step
- Detailed verification
- Easy to debug
- Better for CI/CD

## Example Run

```bash
$ ./test_step_by_step.sh

╔════════════════════════════════════════════════════════════════╗
║        Comprehensive Step-by-Step E2E Test                     ║
║        Testing: Deposit, Withdraw, Order, Cancel               ║
╚════════════════════════════════════════════════════════════════╝

📍 STEP 1: Environment Setup
✅ All processes killed
✅ Logs cleaned
✅ Database cleaned

📍 STEP 2: Building Services
✅ All binaries compiled successfully

📍 STEP 3: Starting Services
✅ UBSCore started (PID: 12345)
✅ Settlement started (PID: 12346)
✅ Matching Engine started (PID: 12347)
✅ Gateway started (PID: 12348)

📍 STEP 4: TEST: Deposit (Transfer In)
✅ Deposit accepted by Gateway
✅ Event logged: DEPOSIT_CONSUMED
✅ Event logged: DEPOSIT_PERSISTED
✅ ✓ Deposit test PASSED

... (continues for all operations)

╔════════════════════════════════════════════════════════════════╗
║                    🎉 TEST COMPLETE! 🎉                        ║
╚════════════════════════════════════════════════════════════════╝

All critical operations tested:
  ✅ Deposit (Transfer In)
  ✅ Withdraw (Transfer Out)
  ✅ Create Order
  ✅ Cancel Order
  ✅ Balance Verification
  ✅ Event Logging
  ✅ Async JSON Logging
```

## CI/CD Integration

Can be used in continuous integration:

```yaml
# .github/workflows/e2e.yml
- name: Run E2E Test
  run: ./test_step_by_step.sh

- name: Upload logs
  if: always()
  uses: actions/upload-artifact@v2
  with:
    name: test-logs
    path: logs/
```

## Next Steps After Test

1. **View detailed logs**:
   ```bash
   ./verify_logging.sh
   ```

2. **Check JSON format**:
   ```bash
   tail logs/ubscore.log | jq .
   ```

3. **Query specific events**:
   ```bash
   grep "event_id=deposit_" logs/*.log
   ```

4. **Monitor real-time**:
   ```bash
   tail -f logs/*.log | jq -C .
   ```

## Summary

This script provides **complete visibility** into each operation with **automatic verification**, making it the perfect tool for:
- Development testing
- CI/CD pipelines
- Debugging issues
- Validating new features
- Demo purposes

**Status**: ✅ Production-ready comprehensive E2E test
