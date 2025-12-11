# TigerBeetle Integration Design

## Overview

This document describes the integration of TigerBeetle as a real-time balance engine alongside ScyllaDB for event storage. TigerBeetle serves as both a validator for Matching Engine correctness and a high-performance balance query layer.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              WRITE PATH                                     │
└─────────────────────────────────────────────────────────────────────────────┘

  Order Input        Matching Engine              Settlement Service
  (Kafka)            (In-Memory)                  (Dual Write)
      │                   │                            │
      ▼                   ▼                            ▼
┌──────────┐        ┌──────────┐        ┌─────────────────────────────────────┐
│validated │───────▶│   ME     │───────▶│         tokio::join!(               │
│_orders   │        │          │        │                                     │
│          │        │GlobalLedger       │           ┌─────────────────────┐   │
│          │        │(in-memory)│       │           │   TigerBeetle       │   │
│          │        │          │        │           │   (balance state)   │◀──┤ Fast, Validated
└──────────┘        └──────────┘        │           └─────────────────────┘   │
                                        │                                     │
                                        │           ┌─────────────────────┐   │
                                        │           │   ScyllaDB          │   │
                                        │           │   (event history)   │◀──┤ Durable, Queryable
                                        │           └─────────────────────┘   │
                                        │                                     │
                                        └─────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                              READ PATH                                      │
└─────────────────────────────────────────────────────────────────────────────┘

                    Gateway API
                        │
        ┌───────────────┼───────────────┐
        ▼               ▼               ▼
  GET /balance    GET /trades     GET /history
        │               │               │
        ▼               ▼               ▼
  TigerBeetle      ScyllaDB        ScyllaDB
  (O(1) lookup)    (trades)        (balance_events)
```

## Data Ownership

| Data Type | TigerBeetle | ScyllaDB | Notes |
|-----------|-------------|----------|-------|
| Current Balance | ✅ Primary | ❌ | Query from TB only |
| Available/Frozen | ✅ Primary | ❌ | TB pending transfers = frozen |
| Balance Events | Derived | ✅ Primary | Append-only audit log |
| Trades | ❌ | ✅ Primary | Full trade history |
| Active Orders | ❌ | ✅ Primary | Order lifecycle |
| User Accounts | ❌ | ✅ Primary | User metadata |

## TigerBeetle as ME Validator

### The Key Insight

Every ME trade output maps 1:1 to a TigerBeetle transfer:

```
ME Trade:                           TigerBeetle Transfer:
┌─────────────────────────┐         ┌─────────────────────────────────────┐
│ trade_id: 12345         │         │ id: 12345                           │
│ buyer: 1001             │────────▶│ credit_account: hash(1001, BTC)     │
│ seller: 1002            │         │ debit_account: hash(1002, BTC)      │
│ symbol: BTC_USDT        │         │ amount: 10_000_000 (0.1 BTC)        │
│ quantity: 0.1 BTC       │         │ user_data_128: kafka_offset         │
│ price: 50000 USDT       │         └─────────────────────────────────────┘
└─────────────────────────┘
                                    ┌─────────────────────────────────────┐
                                    │ id: 12346 (quote leg)               │
                            ───────▶│ credit_account: hash(1002, USDT)    │
                                    │ debit_account: hash(1001, USDT)     │
                                    │ amount: 5_000_000_000 (5000 USDT)   │
                                    │ flags: LINKED (atomic with above)   │
                                    └─────────────────────────────────────┘
```

### Validation Logic

```rust
// If TigerBeetle accepts the transfer:
//   → ME internal state was CORRECT
//   → balance[seller.BTC] >= trade.quantity (proven)

// If TigerBeetle rejects (exceeds_credits):
//   → ME has a BUG
//   → ME allowed a trade that violated balance constraints
//   → HALT and investigate

match tb.create_transfer(trade_transfer).await {
    Ok(_) => {
        // ✅ ME state verified correct
        tracing::info!("Trade {} validated by TigerBeetle", trade.id);
    }
    Err(TransferError::ExceedsCredits) => {
        // ❌ CRITICAL: ME bug detected
        panic!("ME/TB mismatch! ME allowed invalid trade");
    }
}
```

## Account ID Mapping

TigerBeetle uses 128-bit account IDs. We derive them deterministically:

```rust
/// Generate TigerBeetle account ID from (user_id, asset_id)
pub fn tb_account_id(user_id: u64, asset_id: u32) -> u128 {
    // High 64 bits: user_id
    // Low 32 bits: asset_id
    // Remaining 32 bits: reserved (0)
    ((user_id as u128) << 64) | (asset_id as u128)
}

/// Extract user_id from account_id
pub fn extract_user_id(account_id: u128) -> u64 {
    (account_id >> 64) as u64
}

/// Extract asset_id from account_id
pub fn extract_asset_id(account_id: u128) -> u32 {
    (account_id & 0xFFFFFFFF) as u32
}

// Examples:
// User 1001, BTC (asset 1):  tb_account_id(1001, 1) = 0x00000000000003E9_00000001
// User 1001, USDT (asset 2): tb_account_id(1001, 2) = 0x00000000000003E9_00000002
```

## Transfer ID Generation

> ⚠️ **Pro Tip**: Random identifiers have ~10% lower throughput than strictly-increasing IDs
> due to LSM tree optimizations. Use time-based identifiers (ULID/TSID) for best performance.

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TRANSFER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Generate strictly-increasing transfer ID (TSID pattern)
/// High 48 bits: milliseconds since epoch
/// Low 80 bits: sequence counter (allows ~1.2 quintillion IDs per millisecond)
pub fn generate_transfer_id() -> u128 {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u128;

    let sequence = TRANSFER_SEQUENCE.fetch_add(1, Ordering::SeqCst) as u128;

    // Combine: timestamp in high bits, sequence in low bits
    (timestamp_ms << 80) | (sequence & 0xFFFF_FFFF_FFFF_FFFF_FFFF)
}

/// Generate paired transfer IDs for base + quote legs of a trade
pub fn generate_trade_transfer_ids() -> (u128, u128) {
    let base_id = generate_transfer_id();
    let quote_id = generate_transfer_id();
    (base_id, quote_id)
}
```

## Account Creation

Accounts must be created before transfers can reference them. Use a lazy creation
pattern with idempotent handling:

```rust
use tigerbeetle::{Account, AccountFlags, CreateAccountResult};

/// Ensure account exists, creating it if necessary (idempotent)
async fn ensure_account_exists(
    tb: &TigerBeetleClient,
    user_id: u64,
    asset_id: u32,
    ledger_id: u32,
) -> Result<()> {
    let account_id = tb_account_id(user_id, asset_id);

    let account = Account {
        id: account_id,
        user_data_128: user_id as u128,   // Link back to OLGP user record
        user_data_64: asset_id as u64,
        user_data_32: 0,
        ledger: ledger_id,
        code: asset_id as u16,            // Account type = asset ID

        // ⚠️ CRITICAL: This flag enforces balance cannot go negative
        // credits_posted - debits_posted - debits_pending >= 0
        flags: AccountFlags::DEBITS_MUST_NOT_EXCEED_CREDITS,

        ..Default::default()
    };

    match tb.create_accounts(&[account]).await? {
        results if results.iter().all(|r|
            *r == CreateAccountResult::Ok || *r == CreateAccountResult::Exists
        ) => Ok(()),
        results => Err(anyhow!("Account creation failed: {:?}", results)),
    }
}

/// Create accounts for a new user during onboarding
async fn create_user_accounts(
    tb: &TigerBeetleClient,
    user_id: u64,
    asset_ids: &[u32],
    ledger_id: u32,
) -> Result<()> {
    let accounts: Vec<Account> = asset_ids
        .iter()
        .map(|&asset_id| Account {
            id: tb_account_id(user_id, asset_id),
            user_data_128: user_id as u128,
            user_data_64: asset_id as u64,
            user_data_32: 0,
            ledger: ledger_id,
            code: asset_id as u16,
            flags: AccountFlags::DEBITS_MUST_NOT_EXCEED_CREDITS,
            ..Default::default()
        })
        .collect();

    let results = tb.create_accounts(&accounts).await?;

    for (i, result) in results.iter().enumerate() {
        match result {
            CreateAccountResult::Ok => {
                tracing::info!("Created account for user {} asset {}", user_id, asset_ids[i]);
            }
            CreateAccountResult::Exists => {
                tracing::debug!("Account already exists for user {} asset {}", user_id, asset_ids[i]);
            }
            err => {
                tracing::error!("Failed to create account: {:?}", err);
                return Err(anyhow!("Account creation failed: {:?}", err));
            }
        }
    }

    Ok(())
}
```

## Checkpoint & Recovery

TigerBeetle stores the Kafka offset in `user_data_128`, enabling crash recovery:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           CRASH RECOVERY FLOW                               │
└─────────────────────────────────────────────────────────────────────────────┘

  Before Crash:                    After Restart:

  Kafka: [order_1, order_2, ..., order_N, order_N+1, ...]
                                        ▲
                                        │
  TigerBeetle:                          │ Last processed = N
  ┌───────────────────────┐             │
  │ Transfer 1            │             │
  │ Transfer 2            │             │
  │ ...                   │             │
  │ Transfer N            │─────────────┘
  │   user_data_128: N    │ ◀── Checkpoint
  └───────────────────────┘

  Recovery Steps:
  1. Query TB for last transfer's user_data_128 → N
  2. Load all account balances from TB → Initialize ME GlobalLedger
  3. Seek Kafka to offset N+1
  4. Resume processing
```

### Recovery Implementation

```rust
use tigerbeetle::QueryFilter;

const BATCH_SIZE: usize = 8190;  // TigerBeetle max batch size

async fn recover_from_tigerbeetle(
    tb: &TigerBeetleClient,
    me: &mut MatchingEngine,
    kafka: &KafkaConsumer,
    known_account_ids: &[u128],  // All possible (user_id, asset_id) combinations
) -> Result<()> {
    // 1. Get checkpoint (last processed Kafka offset) using query with REVERSED flag
    let last_transfers = tb.query_transfers(&QueryFilter {
        user_data_128: 0,
        user_data_64: 0,
        user_data_32: 0,
        code: 0,
        timestamp_min: 0,
        timestamp_max: u64::MAX,
        limit: 1,
        flags: QueryFilterFlags::REVERSED,  // Most recent first
    }).await?;

    let checkpoint: u64 = last_transfers
        .first()
        .map(|t| t.user_data_128 as u64)
        .unwrap_or(0);

    tracing::info!("Recovering from TigerBeetle checkpoint: {}", checkpoint);

    // 2. Load balances into ME GlobalLedger (paginated in batches of 8190)
    for chunk in known_account_ids.chunks(BATCH_SIZE) {
        let accounts = tb.lookup_accounts(chunk).await?;

        for account in accounts {
            // Skip accounts that don't exist (id will be 0)
            if account.id == 0 { continue; }

            let user_id = extract_user_id(account.id);
            let asset_id = extract_asset_id(account.id);

            // Use saturating_sub to prevent underflow
            let available = account.credits_posted
                .saturating_sub(account.debits_posted)
                .saturating_sub(account.debits_pending);
            let frozen = account.debits_pending;

            me.global_ledger.set_balance(user_id, asset_id, available, frozen);

            tracing::debug!(
                "Restored balance: user={} asset={} available={} frozen={}",
                user_id, asset_id, available, frozen
            );
        }
    }

    // 3. Seek Kafka to resume point
    kafka.seek("validated_orders", checkpoint + 1)?;

    tracing::info!(
        "ME recovered. Loaded {} accounts. Resuming from Kafka offset {}",
        known_account_ids.len(),
        checkpoint + 1
    );
    Ok(())
}
```

## Dual Write Implementation

### Settlement Service Changes

```rust
// settlement_service.rs

async fn process_engine_output(
    output: EngineOutput,
    tb: &TigerBeetleClient,
    scylla: &SettlementDb,
    kafka_offset: u64,
) -> Result<()> {
    // Build TigerBeetle transfers from trades
    let transfers = build_transfers(&output.trades, kafka_offset);

    // Concurrent write to both stores
    let (tb_result, scylla_result) = tokio::join!(
        // TigerBeetle: balance state + validation
        tb.create_transfers(&transfers),

        // ScyllaDB: event history (always durable)
        async {
            scylla.append_trades(&output.trades).await?;
            scylla.append_balance_events(&output.balance_events).await?;
            scylla.update_active_orders(&output.order_placements, &output.order_completions).await
        }
    );

    // Handle results
    match tb_result {
        Ok(_) => {
            tracing::debug!("TB accepted {} transfers", transfers.len());
        }
        Err(e) => {
            // CRITICAL: ME bug detected
            tracing::error!("TB REJECTED transfers: {:?}", e);
            // Option 1: Panic (fail-fast)
            // Option 2: Alert + halt trading
            // Option 3: Mark for manual review
            panic!("TigerBeetle rejected ME output - balance mismatch!");
        }
    }

    scylla_result?; // ScyllaDB must succeed

    Ok(())
}

fn build_transfers(trades: &[TradeOutput], kafka_offset: u64) -> Vec<Transfer> {
    trades.iter().flat_map(|trade| {
        // Generate time-based IDs for optimal LSM throughput
        let (base_id, quote_id) = generate_trade_transfer_ids();

        // Each trade = 2 linked transfers (base + quote assets)
        // LINKED flag: first transfer links to next, both succeed or both fail
        let base_transfer = Transfer {
            id: base_id,
            debit_account_id: tb_account_id(trade.seller_user_id, trade.base_asset_id),
            credit_account_id: tb_account_id(trade.buyer_user_id, trade.base_asset_id),
            amount: trade.quantity,
            user_data_128: kafka_offset as u128,
            flags: TransferFlags::LINKED,  // Atomic with next transfer
            ..Default::default()
        };

        // Quote leg: NO LINKED flag (closes the atomic chain)
        let quote_transfer = Transfer {
            id: quote_id,
            debit_account_id: tb_account_id(trade.buyer_user_id, trade.quote_asset_id),
            credit_account_id: tb_account_id(trade.seller_user_id, trade.quote_asset_id),
            amount: trade.quantity * trade.price / PRICE_SCALE,
            user_data_128: kafka_offset as u128,
            // No LINKED flag - this closes the atomic chain
            ..Default::default()
        };

        vec![base_transfer, quote_transfer]
    }).collect()
}

/// Process transfers in optimal batches (max 8190 per batch)
async fn create_transfers_batched(
    tb: &TigerBeetleClient,
    transfers: &[Transfer],
) -> Result<()> {
    const BATCH_SIZE: usize = 8190;

    for (batch_idx, chunk) in transfers.chunks(BATCH_SIZE).enumerate() {
        let results = tb.create_transfers(chunk).await?;

        // Check individual results
        for (i, result) in results.iter().enumerate() {
            match result {
                CreateTransferResult::Ok => {},
                CreateTransferResult::Exists => {
                    // Idempotent - already processed
                    tracing::debug!("Transfer {} already exists (idempotent)", chunk[i].id);
                }
                err => {
                    // CRITICAL: ME bug or system error
                    tracing::error!(
                        "Batch {} transfer {} failed: {:?}",
                        batch_idx, chunk[i].id, err
                    );
                    return Err(anyhow!("Transfer failed: {:?}", err));
                }
            }
        }

        tracing::debug!("Batch {} processed {} transfers", batch_idx, chunk.len());
    }

    Ok(())
}
```

### Gateway Balance Query

```rust
// gateway.rs

/// GET /api/v1/balance?user_id=1001
async fn get_balances(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<BalanceParams>,
) -> Result<Json<ApiResponse<Vec<BalanceResponse>>>, StatusCode> {
    let user_id = params.user_id;

    // Get all asset IDs for this user (from mapping table or config)
    let asset_ids = state.symbol_manager.get_all_asset_ids();

    // Build account IDs
    let account_ids: Vec<u128> = asset_ids
        .iter()
        .map(|&a| tb_account_id(user_id, a))
        .collect();

    // O(1) lookup in TigerBeetle
    let accounts = state.tb_client
        .lookup_accounts(&account_ids)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let balances: Vec<BalanceResponse> = accounts
        .iter()
        .zip(asset_ids.iter())
        .filter(|(acc, _)| acc.credits_posted > 0 || acc.debits_posted > 0)
        .map(|(acc, &asset_id)| {
            let total = acc.credits_posted - acc.debits_posted;
            let frozen = acc.debits_pending;
            let available = total - frozen;

            BalanceResponse {
                asset: state.symbol_manager.get_asset_name(asset_id).unwrap_or("UNKNOWN".into()),
                asset_id,
                total: format_amount(total, asset_id),
                available: format_amount(available, asset_id),
                frozen: format_amount(frozen, asset_id),
            }
        })
        .collect();

    Ok(Json(ApiResponse::success(balances)))
}
```

## Pending Transfers for Order Holds

TigerBeetle's pending transfer feature maps perfectly to order placement:

```rust
// On order placement: create PENDING transfer (freeze funds)
async fn on_order_placement(order: &OrderPlacement, tb: &TigerBeetleClient) {
    let freeze_transfer = Transfer {
        id: order.order_id,

        // Freeze the quote asset for buy orders, base asset for sell orders
        debit_account_id: match order.side {
            Side::Buy => tb_account_id(order.user_id, QUOTE_ASSET),
            Side::Sell => tb_account_id(order.user_id, BASE_ASSET),
        },
        credit_account_id: EXCHANGE_HOLDING_ACCOUNT,

        amount: calculate_hold_amount(order),

        // PENDING = funds are frozen but not transferred yet
        flags: TransferFlags::PENDING,

        user_data_128: order.order_id as u128,
        ..Default::default()
    };

    tb.create_transfer(&freeze_transfer).await?;
}

// On trade: POST the pending transfer (release frozen → transfer)
async fn on_trade_match(trade: &Trade, tb: &TigerBeetleClient) {
    let post_transfer = Transfer {
        id: trade.trade_id,
        pending_id: trade.order_id, // Links to the pending transfer
        flags: TransferFlags::POST_PENDING_TRANSFER,
        ..Default::default()
    };

    tb.create_transfer(&post_transfer).await?;
}

// On order cancel: VOID the pending transfer (unfreeze)
async fn on_order_cancel(order_id: u64, tb: &TigerBeetleClient) {
    let void_transfer = Transfer {
        id: generate_id(),
        pending_id: order_id,
        flags: TransferFlags::VOID_PENDING_TRANSFER,
        ..Default::default()
    };

    tb.create_transfer(&void_transfer).await?;
}
```

## Deployment

### TigerBeetle Cluster

```yaml
# docker-compose.tigerbeetle.yml
version: '3.8'
services:
  tigerbeetle-0:
    image: ghcr.io/tigerbeetle/tigerbeetle:latest
    command: start --addresses=0.0.0.0:3000,tigerbeetle-1:3000,tigerbeetle-2:3000 0
    volumes:
      - tb-data-0:/data
    ports:
      - "3000:3000"

  tigerbeetle-1:
    image: ghcr.io/tigerbeetle/tigerbeetle:latest
    command: start --addresses=tigerbeetle-0:3000,0.0.0.0:3000,tigerbeetle-2:3000 1
    volumes:
      - tb-data-1:/data

  tigerbeetle-2:
    image: ghcr.io/tigerbeetle/tigerbeetle:latest
    command: start --addresses=tigerbeetle-0:3000,tigerbeetle-1:3000,0.0.0.0:3000 2
    volumes:
      - tb-data-2:/data

volumes:
  tb-data-0:
  tb-data-1:
  tb-data-2:
```

### Config

```yaml
# config/dev.yaml
tigerbeetle:
  addresses:
    - "127.0.0.1:3000"
    - "127.0.0.1:3001"
    - "127.0.0.1:3002"
  cluster_id: 0

  # Special account IDs
  exchange_omnibus_account: 0xFFFFFFFFFFFFFFFF_00000000  # Deposits/Withdrawals
  exchange_holding_account: 0xFFFFFFFFFFFFFFFF_00000001  # Order holds
```

## Implementation Phases

### Phase 1: Foundation (2 days)
- [ ] Add `tigerbeetle-rust` dependency
- [ ] Implement `TigerBeetleClient` wrapper
- [ ] Implement account ID mapping utilities
- [ ] Docker compose for TB cluster

### Phase 2: Dual Write (3 days)
- [ ] Modify Settlement Service for concurrent writes
- [ ] Implement `build_transfers()` for trades
- [ ] Add error handling for TB rejection
- [ ] Integration tests

### Phase 3: Query Integration (2 days)
- [ ] Modify Gateway balance endpoint to use TB
- [ ] Add TB health check endpoint
- [ ] Performance benchmarks

### Phase 4: Recovery (2 days)
- [ ] Implement `recover_from_tigerbeetle()`
- [ ] Add checkpoint tracking in transfers
- [ ] Recovery integration tests

### Phase 5: Pending Transfers (2 days)
- [ ] Implement order hold on placement
- [ ] Implement release on trade match
- [ ] Implement void on cancel

## Benefits Summary

| Before | After |
|--------|-------|
| Balance query: aggregate events | Balance query: O(1) lookup |
| Validation: trust ME | Validation: TB enforces |
| Recovery: replay entire WAL | Recovery: load TB snapshot |
| Consistency: hope | Consistency: proven |
| Order holds: manual tracking | Order holds: TB pending transfers |

## Appendix: TigerBeetle Error Codes

| Error | Meaning | Action |
|-------|---------|--------|
| `exceeds_credits` | Insufficient balance | ME bug - halt |
| `exceeds_debits` | Credit overflow | ME bug - halt |
| `exists` | Duplicate transfer ID | Skip (idempotent) |
| `accounts_must_be_different` | Self-transfer | ME bug - halt |
| `linked_event_failed` | Atomic batch failed | Retry or halt |

---

## Best Practices Summary

> These recommendations are based on official TigerBeetle documentation and
> production-grade deployment patterns.

### Performance Optimization

| Practice | Why |
|----------|-----|
| Use time-based IDs (ULID/TSID) | ~10% higher throughput vs random IDs due to LSM optimizations |
| Batch transfers (max 8190) | TigerBeetle is optimized for batch processing |
| Cache asset mappings | Never fetch from OLGP DB during transfer path |
| Use `saturating_sub()` | Prevent underflow when calculating available balance |

### Safety Guarantees

| Practice | Why |
|----------|-----|
| `DEBITS_MUST_NOT_EXCEED_CREDITS` flag | TigerBeetle enforces balance >= 0 atomically |
| Fail-fast on TB rejection | If TB rejects, ME has a bug - halt and investigate |
| Check individual transfer results | Handle `Exists` (idempotent) vs real errors |
| Store Kafka offset in `user_data_128` | Enables exact-once recovery |

### Division of Responsibilities

| Component | Responsibility |
|-----------|----------------|
| **App/Gateway** | Auth, ID generation, batching, rate limiting |
| **ScyllaDB (OLGP)** | User metadata, event history, trade records |
| **TigerBeetle (OLTP)** | Balance state, transfer records, constraint enforcement |
| **Matching Engine** | In-memory order matching, price-time priority |

### Account Management

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        ACCOUNT LIFECYCLE                                    │
└─────────────────────────────────────────────────────────────────────────────┘

  User Signup              First Trade/Deposit          Trading
      │                          │                        │
      ▼                          ▼                        ▼
  ┌──────────┐              ┌──────────┐            ┌──────────┐
  │ Create   │──── OR ──────│ Lazy     │            │ Lookup   │
  │ accounts │              │ create   │            │ balance  │
  │ (batch)  │              │ if !exist│            │ O(1)     │
  └──────────┘              └──────────┘            └──────────┘

  Always use: AccountFlags::DEBITS_MUST_NOT_EXCEED_CREDITS
```

### Future Enhancements

- **Change Data Capture (CDC)**: Stream TB events to Kafka for analytics
- **Multi-ledger**: Separate ledgers per currency for isolation
- **Rate limiting via TB**: Use `code` field for transfer type limits

---

*Document created: 2025-12-11*
*Updated: 2025-12-11 (Pro-level recommendations added)*
*Status: Design Complete - Ready for Implementation*
