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
async fn recover_from_tigerbeetle(
    tb: &TigerBeetleClient,
    me: &mut MatchingEngine,
    kafka: &KafkaConsumer,
) -> Result<()> {
    // 1. Get checkpoint (last processed Kafka offset)
    let last_transfer = tb.query_transfers(
        .filter(.timestamp, .max),
        .limit(1),
    ).await?;

    let checkpoint: u64 = last_transfer
        .first()
        .map(|t| t.user_data_128 as u64)
        .unwrap_or(0);

    tracing::info!("Recovering from TigerBeetle checkpoint: {}", checkpoint);

    // 2. Load balances into ME GlobalLedger
    let accounts = tb.lookup_accounts(&[/* all known account IDs */]).await?;

    for account in accounts {
        let user_id = extract_user_id(account.id);
        let asset_id = extract_asset_id(account.id);
        let available = account.credits_posted - account.debits_posted - account.debits_pending;
        let frozen = account.debits_pending;

        me.global_ledger.set_balance(user_id, asset_id, available, frozen);
    }

    // 3. Seek Kafka to resume point
    kafka.seek("validated_orders", checkpoint + 1)?;

    tracing::info!("ME recovered. Resuming from Kafka offset {}", checkpoint + 1);
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
        // Each trade = 2 linked transfers (base + quote assets)
        let base_transfer = Transfer {
            id: trade.trade_id,
            debit_account_id: tb_account_id(trade.seller_user_id, trade.base_asset_id),
            credit_account_id: tb_account_id(trade.buyer_user_id, trade.base_asset_id),
            amount: trade.quantity,
            user_data_128: kafka_offset as u128,
            flags: TransferFlags::LINKED,
            ..Default::default()
        };

        let quote_transfer = Transfer {
            id: trade.trade_id + 1, // Or use separate ID generator
            debit_account_id: tb_account_id(trade.buyer_user_id, trade.quote_asset_id),
            credit_account_id: tb_account_id(trade.seller_user_id, trade.quote_asset_id),
            amount: trade.quantity * trade.price / PRICE_SCALE,
            user_data_128: kafka_offset as u128,
            ..Default::default()
        };

        vec![base_transfer, quote_transfer]
    }).collect()
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

*Document created: 2025-12-11*
*Status: Design Complete - Ready for Implementation*
