# End-to-End Testing Guide: Deposit/Withdrawal System

## Overview

This guide walks you through testing the complete deposit/withdrawal flow from start to finish.

## Architecture Components

```
balance_test_client → deposit_withdraw_gateway → Kafka → balance_processor → MatchingEngine
                      (HTTP :8082)                (topic)  (validates)        (ledger)
```

## Prerequisites

1. **Redpanda/Kafka** running on `localhost:9093`
2. **All binaries built**

## Step-by-Step Testing

### Step 1: Start Redpanda

```bash
# If not already running
docker run -d --name redpanda \
  -p 9092:9092 \
  -p 9093:9093 \
  docker.redpanda.com/redpandadata/redpanda:latest \
  redpanda start --smp 1 --memory 1G
```

**Verify:**
```bash
docker ps | grep redpanda
# Should show running container
```

### Step 2: Build All Binaries

```bash
cargo build --release \
  --bin deposit_withdraw_gateway \
  --bin balance_processor \
  --bin balance_test_client
```

**Expected:** All compile successfully

### Step 3: Start Deposit/Withdraw Gateway (Terminal 1)

```bash
cargo run --bin deposit_withdraw_gateway --release
```

**Expected Output:**
```
--------------------------------------------------
Deposit/Withdraw Gateway Started (Simplified)
  Listening on:      0.0.0.0:8082
  Kafka Broker:      localhost:9093
  Balance Topic:     balance.operations
  Time Window:       60 seconds
--------------------------------------------------
Endpoints:
  POST /api/v1/deposit   - Transfer from funding_account to user
  POST /api/v1/withdraw  - Transfer from user to funding_account
  GET  /health           - Health check
--------------------------------------------------
```

**Test Health:**
```bash
curl http://localhost:8082/health
# Should return: {"success":true,"message":"...","request_id":null}
```

### Step 4: Start Balance Processor (Terminal 2)

```bash
cargo run --bin balance_processor --release
```

**Expected Output:**
```
--------------------------------------------------
Balance Processor Started (Direct ME Integration)
  Kafka Broker:      localhost:9093
  Balance Topic:     balance.operations
  Consumer Group:    balance_processor_group
  WAL Directory:     "me_wal_data"
  Snapshot Dir:      "me_snapshots"
  Acceptance Window: 60 seconds (requests older than this are rejected)
  Tracking Window:   300 seconds (prevents replay attacks)
--------------------------------------------------
Architecture:
  Funding Account:   Simulated (Database in production)
  Trading Accounts:  MatchingEngine.ledger
  Integration:       Direct function call
--------------------------------------------------
```

### Step 5: Run Test Client (Terminal 3)

```bash
cargo run --bin balance_test_client --release
```

**Expected Output:**
```
=== Simplified Deposit/Withdraw Test Client ===

All transfers are internal: funding_account <-> trading_account
Time window: 60 seconds
Using ULID for unique request_id

📥 Test 1: Deposit (funding_account → user 1001)
Response: 200 OK
Body: {"success":true,"message":"Deposit request submitted...","request_id":"01HXX..."}

📥 Test 2: Deposit (funding_account → user 1002)
Response: 200 OK
...

📤 Test 3: Withdrawal (user 1001 → funding_account)
Response: 200 OK
...

📥 Test 4: Duplicate Deposit (Idempotency Test)
First request: 200 OK
Duplicate request: 200 OK
...

📥 Test 5: Rapid Deposits (Stress Test - Unique ULIDs)
  Deposit 0 (01HXX...): 200 OK
  Deposit 1 (01HXY...): 200 OK
  ...

🏥 Test 6: Health Check
Response: 200 OK

=== All tests completed ===
```

### Step 6: Verify Balance Processor Logs (Terminal 2)

You should see detailed processing logs:

**Test 1 - Deposit:**
```
🔍 Validating request: 01HXX...
   Request timestamp: 1701600000000
   Current time:      1701600005000
   ✓ Timestamp valid (within 60s window)
   ✓ Request ID is unique (not seen before)

📥 Processing Deposit: 100000000 units of asset 1 for user 1001
   ✓ Reserved 100000000 from funding account
✅ Deposit completed: 01HXX...
   Transferred 100000000 from funding_account to user 1001's trading account
```

**Test 3 - Withdrawal:**
```
🔍 Validating request: 01HXZ...
   ✓ Timestamp valid (within 60s window)
   ✓ Request ID is unique (not seen before)

📤 Processing Withdrawal: 50000000 units of asset 1 from user 1001
   ✓ Withdrawn 50000000 from user 1001's trading account
✅ Withdrawal completed: 01HXZ...
   Transferred 50000000 from user 1001's trading account to funding_account
```

**Test 4 - Duplicate Detection:**
```
🔍 Validating request: 01HXA...
   ✓ Timestamp valid (within 60s window)
   ✓ Request ID is unique (not seen before)
📥 Processing Deposit: ...
✅ Deposit completed: 01HXA...

🔍 Validating request: 01HXA...  (Same ID!)
   ✓ Timestamp valid (within 60s window)
❌ REJECTED: Duplicate request detected: 01HXA... (first seen 2s ago)
   ⚠️  This request_id was already processed. Ignoring to prevent double-spend.
```

## Manual Testing with curl

### Test 1: Simple Deposit

```bash
curl -X POST http://localhost:8082/api/v1/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "manual_deposit_001",
    "user_id": 2001,
    "asset_id": 1,
    "amount": 50000000
  }'
```

**Expected Response:**
```json
{
  "success": true,
  "message": "Deposit request submitted: 50000000 units of asset 1 will be transferred to user 2001",
  "request_id": "manual_deposit_001"
}
```

**Check balance_processor logs** - should show deposit processing

### Test 2: Withdrawal

```bash
curl -X POST http://localhost:8082/api/v1/withdraw \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "manual_withdraw_001",
    "user_id": 2001,
    "asset_id": 1,
    "amount": 25000000
  }'
```

**Expected:** Success if user has balance from Test 1

### Test 3: Duplicate Detection

```bash
# Send same request again
curl -X POST http://localhost:8082/api/v1/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "manual_deposit_001",
    "user_id": 2001,
    "asset_id": 1,
    "amount": 50000000
  }'
```

**Expected in balance_processor:**
```
❌ REJECTED: Duplicate request detected: manual_deposit_001
```

### Test 4: Insufficient Balance

```bash
curl -X POST http://localhost:8082/api/v1/withdraw \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "manual_withdraw_002",
    "user_id": 2001,
    "asset_id": 1,
    "amount": 999999999999
  }'
```

**Expected in balance_processor:**
```
❌ Failed to withdraw from trading account: Insufficient funds
```

## Monitoring Kafka

### View Messages in Kafka Topic

```bash
docker exec -it redpanda rpk topic consume balance.operations
```

**Expected Output:**
```json
{
  "type": "Deposit",
  "data": {
    "request_id": "01HXX...",
    "user_id": 1001,
    "asset_id": 1,
    "amount": 100000000,
    "timestamp": 1701600000000
  }
}
```

### Check Topic Stats

```bash
docker exec -it redpanda rpk topic describe balance.operations
```

## Troubleshooting

### Issue: Gateway returns 500 error

**Check:**
1. Is Redpanda running? `docker ps | grep redpanda`
2. Can gateway connect? Check gateway logs for Kafka errors

**Fix:**
```bash
# Restart Redpanda
docker restart redpanda
# Wait 5 seconds
sleep 5
# Restart gateway
```

### Issue: Balance processor not receiving messages

**Check:**
1. Is balance processor subscribed? Check startup logs
2. Are messages in Kafka? `rpk topic consume balance.operations`

**Fix:**
```bash
# Check consumer group
docker exec -it redpanda rpk group describe balance_processor_group

# Reset offsets if needed
docker exec -it redpanda rpk group seek balance_processor_group \
  --to start --topics balance.operations
```

### Issue: Duplicate not detected

**Possible causes:**
1. Request IDs are different (check logs)
2. More than 5 minutes passed (tracking window expired)
3. Balance processor restarted (in-memory tracking lost)

**Verify:**
```bash
# Check balance_processor logs for:
"✓ Request ID is unique (not seen before)"  # First time
"❌ REJECTED: Duplicate request detected"    # Second time
```

### Issue: Withdrawal fails with "Insufficient funds"

**Check:**
1. Did deposit succeed? Check balance_processor logs
2. Is user_id correct?
3. Is amount correct?

**Debug:**
```bash
# Add logging to see current balance
# Or check ledger WAL files in me_wal_data/
```

## Performance Testing

### Rapid Deposits (Stress Test)

```bash
# Run test client multiple times
for i in {1..10}; do
  cargo run --bin balance_test_client --release &
done
wait
```

**Expected:**
- All requests processed
- No duplicates (each has unique ULID)
- Balance processor handles concurrency

### Check Stats

Balance processor prints stats every 10 seconds:
```
📊 Stats: 150 requests processed, 145 in tracking window
```

## Verification Checklist

- [ ] Gateway starts and responds to /health
- [ ] Balance processor starts and connects to Kafka
- [ ] Test client completes all 6 tests successfully
- [ ] Deposit shows in balance_processor logs
- [ ] Withdrawal shows in balance_processor logs
- [ ] Duplicate detection works (same request_id rejected)
- [ ] Insufficient balance rejection works
- [ ] Messages visible in Kafka topic
- [ ] Stats show correct counts
- [ ] No errors in any terminal

## Success Criteria

✅ **All tests pass**
✅ **Deposits processed correctly**
✅ **Withdrawals processed correctly**
✅ **Duplicates detected and rejected**
✅ **Insufficient balance handled**
✅ **No crashes or errors**

## Next Steps

After successful testing:

1. **Integration Testing**
   - Test with real matching engine
   - Test concurrent deposits/withdrawals
   - Test edge cases

2. **Load Testing**
   - 1000 deposits/sec
   - 1000 withdrawals/sec
   - Mixed workload

3. **Failure Testing**
   - Kill balance_processor mid-request
   - Kill gateway mid-request
   - Kafka unavailable

4. **Production Readiness**
   - Replace simulated funding account with database
   - Add authentication to gateway
   - Add monitoring/alerting
   - Add rate limiting

## Quick Test Script

Save this as `test_deposit_withdraw.sh`:

```bash
#!/bin/bash

echo "=== Starting End-to-End Test ==="

# Check Redpanda
echo "1. Checking Redpanda..."
docker ps | grep redpanda || {
    echo "❌ Redpanda not running!"
    exit 1
}
echo "✅ Redpanda running"

# Check Gateway
echo "2. Checking Gateway..."
curl -s http://localhost:8082/health | grep -q success || {
    echo "❌ Gateway not responding!"
    exit 1
}
echo "✅ Gateway healthy"

# Run test client
echo "3. Running test client..."
cargo run --bin balance_test_client --release

echo "=== Test Complete ==="
```

Run with:
```bash
chmod +x test_deposit_withdraw.sh
./test_deposit_withdraw.sh
```
