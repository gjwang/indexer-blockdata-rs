# Deposit & Withdrawal Architecture Design

## Current Architecture Analysis

### Existing Components
1. **Ledger System** - Has `Deposit` and `Withdraw` commands in `LedgerCommand` enum
2. **Order Gateway** - Handles order placement via Kafka
3. **Matching Engine** - Consumes orders from Kafka, processes them
4. **Centrifugo Publisher** - Publishes balance updates to users via WebSocket

### Current Flow (Orders)
```
User → order_gate_server → Redpanda (orders topic) → matching_engine_server → Ledger
                                                                              ↓
                                                                    Centrifugo (balance updates)
```

## Proposed Architecture for Deposits/Withdrawals

### Design Option 1: Separate Deposit/Withdrawal Gateway (Recommended) ✅

#### Architecture
```
┌─────────────────────────────────────────────────────────────────┐
│                     External Systems                             │
│  (Blockchain Nodes, Payment Processors, Bank APIs)              │
└────────────────┬────────────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────┐
│              deposit_withdraw_gateway                            │
│  - Validates deposit/withdrawal requests                         │
│  - Checks external confirmations (blockchain, etc.)              │
│  - Publishes to Kafka                                            │
└────────────────┬────────────────────────────────────────────────┘
                 │
                 ▼
         Redpanda (balance_ops topic)
                 │
                 ▼
┌─────────────────────────────────────────────────────────────────┐
│           balance_processor (new service)                        │
│  - Consumes from balance_ops topic                               │
│  - Applies deposits/withdrawals to Ledger                        │
│  - Publishes balance updates to Centrifugo                       │
│  - Logs to Balance WAL (audit trail)                             │
└─────────────────────────────────────────────────────────────────┘
```

#### Why Separate Service?
1. **Isolation** - Balance operations don't interfere with order matching
2. **Scalability** - Can scale deposit/withdrawal processing independently
3. **Security** - Separate authentication/authorization for fund operations
4. **Audit** - Dedicated WAL for financial operations
5. **Recovery** - Independent recovery mechanism

---

### Design Option 2: Unified Gateway (Simpler, but less flexible)

#### Architecture
```
User → unified_gateway → Redpanda → matching_engine_server
       (orders + balance ops)        (handles both)
```

**Pros:** Simpler architecture, fewer services
**Cons:** Couples balance operations with trading, harder to scale independently

---

## Recommended Implementation (Option 1)

### 1. Define Balance Operation Messages

```rust
// src/models/balance_requests.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum BalanceRequest {
    Deposit {
        request_id: String,        // Unique idempotency key
        user_id: u64,
        asset_id: u32,
        amount: u64,
        source: DepositSource,     // Blockchain, Bank, etc.
        external_tx_id: String,    // Blockchain tx hash, bank ref, etc.
        confirmations: u32,        // For blockchain deposits
    },
    Withdraw {
        request_id: String,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        destination: WithdrawDestination,
        external_address: String,  // Blockchain address, bank account, etc.
    },
    WithdrawConfirm {
        request_id: String,        // Confirms withdrawal was sent
        external_tx_id: String,
    },
    WithdrawReject {
        request_id: String,        // Rejects and refunds
        reason: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum DepositSource {
    Blockchain { chain: String, confirmations: u32 },
    BankTransfer { reference: String },
    Internal { from_user_id: u64 },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum WithdrawDestination {
    Blockchain { chain: String, address: String },
    BankTransfer { account: String },
    Internal { to_user_id: u64 },
}
```

### 2. Create Balance Processor Service

```rust
// src/bin/balance_processor.rs

use fetcher::ledger::{GlobalLedger, LedgerCommand};
use fetcher::models::BalanceRequest;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;

struct BalanceProcessor {
    ledger: GlobalLedger,
    consumer: StreamConsumer,
    processed_requests: FxHashSet<String>, // Idempotency tracking
}

impl BalanceProcessor {
    async fn process_balance_request(&mut self, req: BalanceRequest) -> Result<()> {
        match req {
            BalanceRequest::Deposit {
                request_id,
                user_id,
                asset_id,
                amount,
                source,
                external_tx_id,
                confirmations,
            } => {
                // 1. Check idempotency
                if self.processed_requests.contains(&request_id) {
                    println!("Duplicate deposit request: {}", request_id);
                    return Ok(());
                }

                // 2. Validate deposit (check confirmations, etc.)
                if !self.validate_deposit(&source, confirmations) {
                    println!("Deposit validation failed: {}", request_id);
                    return Ok(());
                }

                // 3. Apply to ledger
                self.ledger.apply(&LedgerCommand::Deposit {
                    user_id,
                    asset: asset_id,
                    amount,
                })?;

                // 4. Mark as processed
                self.processed_requests.insert(request_id.clone());

                // 5. Publish balance update to Centrifugo
                self.publish_balance_update(user_id, asset_id).await?;

                println!("Deposit processed: {} for user {}", amount, user_id);
            }

            BalanceRequest::Withdraw {
                request_id,
                user_id,
                asset_id,
                amount,
                destination,
                external_address,
            } => {
                // 1. Check idempotency
                if self.processed_requests.contains(&request_id) {
                    println!("Duplicate withdraw request: {}", request_id);
                    return Ok(());
                }

                // 2. Check balance
                let balance = self.ledger.get_balance(user_id, asset_id);
                if balance < amount {
                    println!("Insufficient balance for withdrawal: {}", request_id);
                    return Err(anyhow::anyhow!("Insufficient balance"));
                }

                // 3. Lock funds (freeze until external tx confirms)
                self.ledger.apply(&LedgerCommand::Lock {
                    user_id,
                    asset: asset_id,
                    amount,
                })?;

                // 4. Mark as processed (pending external confirmation)
                self.processed_requests.insert(request_id.clone());

                // 5. Trigger external withdrawal (blockchain tx, bank transfer)
                self.initiate_external_withdrawal(&destination, amount, &external_address).await?;

                println!("Withdrawal locked: {} for user {}", amount, user_id);
            }

            BalanceRequest::WithdrawConfirm {
                request_id,
                external_tx_id,
            } => {
                // External withdrawal confirmed, spend the frozen funds
                // (Implementation depends on how you track pending withdrawals)
                println!("Withdrawal confirmed: {}", request_id);
            }

            BalanceRequest::WithdrawReject {
                request_id,
                reason,
            } => {
                // Unlock the frozen funds
                println!("Withdrawal rejected: {} - {}", request_id, reason);
            }
        }

        Ok(())
    }

    fn validate_deposit(&self, source: &DepositSource, confirmations: u32) -> bool {
        match source {
            DepositSource::Blockchain { chain, confirmations: required } => {
                // Check if blockchain confirmations meet threshold
                confirmations >= *required
            }
            DepositSource::BankTransfer { .. } => {
                // Validate bank transfer (check with payment processor)
                true
            }
            DepositSource::Internal { .. } => {
                // Internal transfers are pre-validated
                true
            }
        }
    }

    async fn initiate_external_withdrawal(
        &self,
        destination: &WithdrawDestination,
        amount: u64,
        address: &str,
    ) -> Result<()> {
        // This would interact with blockchain nodes, payment processors, etc.
        // For now, just log
        println!("Initiating external withdrawal: {} to {}", amount, address);
        Ok(())
    }

    async fn publish_balance_update(&self, user_id: u64, asset_id: u32) -> Result<()> {
        // Publish to Centrifugo
        // Similar to RedpandaTradeProducer in matching_engine_server.rs
        Ok(())
    }
}
```

### 3. Create Deposit/Withdraw Gateway

```rust
// src/bin/deposit_withdraw_gateway.rs

use axum::{
    extract::Extension,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use rdkafka::producer::{FutureProducer, FutureRecord};
use fetcher::models::BalanceRequest;

#[derive(Clone)]
struct AppState {
    producer: FutureProducer,
    topic: String,
}

async fn deposit(
    Extension(state): Extension<AppState>,
    Json(req): Json<BalanceRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 1. Validate request
    // 2. Check authentication
    // 3. Publish to Kafka

    let payload = serde_json::to_string(&req)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    state
        .producer
        .send(
            FutureRecord::to(&state.topic)
                .payload(&payload)
                .key(&req.user_id().to_string()),
            std::time::Duration::from_secs(0),
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "request_id": req.request_id()
    })))
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/api/v1/deposit", post(deposit))
        .route("/api/v1/withdraw", post(withdraw));

    // Run server...
}
```

### 4. Update Kafka Topics Configuration

```yaml
# config.yaml
kafka:
  topics:
    orders: "orders"
    trades: "trades"
    balance_ops: "balance_ops"  # NEW: For deposits/withdrawals
```

### 5. Add Balance WAL for Audit Trail

```rust
// Similar to order_wal.rs, create balance_wal.rs
// This provides an independent audit trail for all balance operations

pub struct BalanceWal {
    writer: BufWriter<File>,
}

impl BalanceWal {
    pub fn log_deposit(&mut self, request_id: &str, user_id: u64, asset_id: u32, amount: u64) -> Result<()> {
        // Write to WAL using FlatBuffers
    }

    pub fn log_withdraw(&mut self, request_id: &str, user_id: u64, asset_id: u32, amount: u64) -> Result<()> {
        // Write to WAL using FlatBuffers
    }
}
```

---

## Recovery Scenarios

### Scenario 1: Balance Processor Crash

**Recovery:**
```
1. Load latest Ledger snapshot
2. Replay Ledger WAL (gets all balance changes)
3. Replay Balance WAL (gets deposit/withdrawal audit trail)
4. Resume consuming from Kafka (balance_ops topic)
   - Use idempotency keys to skip already-processed requests
```

### Scenario 2: Deposit Confirmation Delay

**Handling:**
```
1. Deposit request arrives with insufficient confirmations
2. Store in pending_deposits table/cache
3. Blockchain monitor service checks confirmations
4. When threshold met, re-publish to balance_ops topic
5. Balance processor applies deposit
```

### Scenario 3: Withdrawal Failure

**Handling:**
```
1. Withdrawal locks funds
2. External tx fails (blockchain rejection, etc.)
3. WithdrawReject message published to balance_ops
4. Balance processor unlocks funds
5. User notified via Centrifugo
```

---

## Security Considerations

### 1. Idempotency
- Use `request_id` as idempotency key
- Store processed requests in persistent storage
- Prevent duplicate deposits/withdrawals

### 2. Two-Factor Authentication
- Require 2FA for withdrawals
- Validate in gateway before publishing to Kafka

### 3. Withdrawal Limits
- Daily/monthly withdrawal limits per user
- Velocity checks (e.g., max 3 withdrawals per hour)

### 4. Hot/Cold Wallet Management
- Small amounts in hot wallet (fast withdrawals)
- Large amounts in cold wallet (manual approval)

### 5. Audit Trail
- Complete audit trail in Balance WAL
- Immutable log of all balance operations
- Regulatory compliance (AML/KYC)

---

## Performance Considerations

### 1. Batch Processing
```rust
// Process deposits in batches
let deposits = collect_batch_from_kafka(100);
ledger.commit_batch(&deposits)?;
```

### 2. Async External Calls
- Don't block on blockchain confirmations
- Use async workers to monitor confirmations

### 3. Caching
- Cache pending deposits/withdrawals
- Reduce database queries

---

## Migration Path

### Phase 1: Add Balance Request Models
- Define `BalanceRequest` enum
- Add to models module

### Phase 2: Create Balance Processor
- Implement basic deposit/withdrawal handling
- Test with manual Kafka messages

### Phase 3: Create Gateway
- Implement deposit/withdraw HTTP endpoints
- Add authentication/authorization

### Phase 4: Add Monitoring
- Blockchain confirmation monitoring
- External payment processor integration

### Phase 5: Production Deployment
- Deploy balance_processor
- Deploy deposit_withdraw_gateway
- Monitor and tune performance

---

## Testing Strategy

### Unit Tests
```rust
#[test]
fn test_deposit_idempotency() {
    // Ensure duplicate deposits are rejected
}

#[test]
fn test_withdraw_insufficient_balance() {
    // Ensure withdrawals fail with insufficient balance
}

#[test]
fn test_withdraw_lock_unlock() {
    // Test withdrawal lock and unlock flow
}
```

### Integration Tests
```rust
#[tokio::test]
async fn test_deposit_flow() {
    // 1. Publish deposit to Kafka
    // 2. Verify ledger updated
    // 3. Verify balance update published to Centrifugo
}
```

### Load Tests
- Test 10k deposits/sec
- Test concurrent deposits for same user
- Test recovery after crash

---

## Conclusion

**Recommended Approach:**
1. ✅ Create separate `balance_processor` service
2. ✅ Use Kafka for async processing
3. ✅ Maintain separate Balance WAL for audit
4. ✅ Implement idempotency for safety
5. ✅ Use Centrifugo for real-time balance updates

This architecture provides:
- **Isolation** - Balance ops don't affect trading
- **Scalability** - Independent scaling
- **Auditability** - Complete audit trail
- **Reliability** - Idempotent processing
- **Performance** - Async, batched processing
