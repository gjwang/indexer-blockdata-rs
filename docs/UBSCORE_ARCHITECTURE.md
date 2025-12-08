# UBSCore: User Balance Service Core Architecture

## Naming Convention

| Old Name | New Name | Role |
|----------|----------|------|
| Risk Engine | **UBSCore** | In-memory balance authority |
| Settlement Service | **Offshore** | Cold database, blockchain |

**UBSCore** = User Balance Service Core
- **UBS**: It holds the money
- **Core**: It decides if money can be spent

## The Golden Rule

> "UBSCore never asks. It knows."

- Never queries a database
- Never calls an external API
- If data isn't in `self.ram`, the data doesn't exist

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GATEWAY                                         │
│                          (Dumb Router)                                       │
│                                                                              │
│   - Auth, Rate Limit, JSON→Binary                                           │
│   - Routes by user_id to correct UBSCore shard                              │
│   - Routes by symbol_id to correct ME (after UBSCore approval)              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Route by user_id % shard_count
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           UBSCore (THE BRAIN)                                │
│                    Sharded by user_id, In-Memory                             │
│                                                                              │
│   ┌───────────────────────────────────────────────────────────────────────┐ │
│   │  HashMap<UserId, Account>                                             │ │
│   │                                                                       │ │
│   │  Account {                                                            │ │
│   │      available: u64,     // Can be used for new orders               │ │
│   │      locked: u64,        // Reserved for pending orders              │ │
│   │      speculative: u64,   // Hot path credits (dirty)                 │ │
│   │      version: u64,       // For optimistic concurrency               │ │
│   │  }                                                                    │ │
│   └───────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│   "UBSCore receives ONLY valid orders. ME never wastes CPU on bad orders."  │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Only approved orders!
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                       MATCHING ENGINE (THE MUSCLE)                           │
│                         (Sharded by symbol_id)                               │
│                                                                              │
│   - NO balance checking (trusts UBSCore 100%)                               │
│   - Pure order book matching                                                 │
│   - Protected from DDoS by empty-wallet users                               │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Multi-Core Architecture (Spot vs Futures)

### Why Separate?

| Market | Computation | Risk |
|--------|-------------|------|
| Spot | Simple: `Balance - Cost` | Very fast |
| Futures | Complex: MarkPrice, Margin, PnL, Funding | Heavy CPU |

**If combined**: Massive futures liquidation could lag the spot market.
**Separated**: Each ecosystem is protected.

### The "Island Chain" Pattern (Binance, Bybit)

```
┌───────────────────────────────────────────────────────────────────────────────┐
│                                GATEWAY                                         │
└───────────────────────────────────────────────────────────────────────────────┘
          │                                             │
          │ user_id % spot_shards                       │ user_id % future_shards
          ▼                                             ▼
┌─────────────────────────────────┐     ┌─────────────────────────────────────┐
│     UBSCore_Spot Cluster        │     │       UBSCore_Futures Cluster       │
│                                 │     │                                     │
│  ┌─────────┐ ┌─────────┐       │     │  ┌─────────┐ ┌─────────┐           │
│  │ Shard 0 │ │ Shard 1 │ ...   │     │  │ Shard 0 │ │ Shard 1 │ ...       │
│  └────┬────┘ └────┬────┘       │     │  └────┬────┘ └────┬────┘           │
│       │           │             │     │       │           │                 │
│  User has:        │             │     │  User has:        │                 │
│  Spot_USDT: 100   │             │     │  Future_USDT: 50  │                 │
│                   │             │     │                   │                 │
└─────────┬─────────┼─────────────┘     └───────────────────┼─────────────────┘
          │         │                                       │
          ▼         ▼                                       ▼
┌─────────────────────────────────┐     ┌─────────────────────────────────────┐
│   ME_Spot_BTC    ME_Spot_ETH    │     │  ME_Future_BTC    ME_Future_ETH    │
└─────────────────────────────────┘     └─────────────────────────────────────┘
```

**User sees**: Two separate balances (Spot_USDT, Future_USDT)
**Trade-off**: User must manually "Transfer" between them

### Inter-Island Ferry (Atomic Transfer)

Moving funds from Spot to Futures without touching the slow database:

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ User: "Transfer 100 USDT from Spot to Futures"                               │
└──────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ Gateway → UBSCore_Spot: TransferOut { user, qty: 100, dest: Futures }        │
└──────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ UBSCore_Spot:                                                                 │
│   1. Check balance: available >= 100? YES                                    │
│   2. Decrement: available -= 100                                             │
│   3. Emit to Redpanda: OutboundTransfer { tx_id: UUID, to: Futures, qty: 100}│
└──────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Redpanda (Topic: internal-transfers)
                                    ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ UBSCore_Futures:                                                              │
│   1. Consume OutboundTransfer                                                │
│   2. Increment: available += 100                                             │
│   3. Emit: TransferAck { tx_id: UUID, status: OK }                          │
└──────────────────────────────────────────────────────────────────────────────┘

Latency: ~2-5ms (Redpanda round trip)
Safety: 100% (Event persisted before ack)
```

## Deposit Flow (Blockchain → UBSCore)

```
┌─────────┐    ┌──────────┐    ┌─────────┐    ┌──────────┐    ┌─────────┐
│  User   │    │Blockchain│    │ Payment │    │ Redpanda │    │ UBSCore │
│         │    │  (BTC)   │    │ Gateway │    │          │    │  (RAM)  │
└────┬────┘    └────┬─────┘    └────┬────┘    └────┬─────┘    └────┬────┘
     │              │               │               │               │
     │ Send 1 BTC   │               │               │               │
     │─────────────▶│               │               │               │
     │              │ Confirmed     │               │               │
     │              │──────────────▶│               │               │
     │              │               │               │               │
     │              │               │ 1. Write to   │               │
     │              │               │    Cold DB    │               │
     │              │               │    (Audit)    │               │
     │              │               │               │               │
     │              │               │ 2. Emit Event │               │
     │              │               │──────────────▶│               │
     │              │               │               │ Deposit(user, │
     │              │               │               │   1 BTC,      │
     │              │               │               │   seq=50)     │
     │              │               │               │──────────────▶│
     │              │               │               │               │
     │              │               │               │ 3. Update RAM │
     │              │               │               │ available+=1.0│
     │              │               │               │               │
     │              │               │               │ User can now  │
     │              │               │               │ trade!        │
     │              │               │               │               │
```

### Idempotency (Prevent Double-Credit)

```rust
struct Account {
    available: u64,
    locked: u64,

    // Track processed deposit IDs
    processed_deposit_ids: HashSet<u64>,
}

impl Account {
    fn apply_deposit(&mut self, deposit_id: u64, amount: u64) {
        // Idempotency check
        if self.processed_deposit_ids.contains(&deposit_id) {
            log::warn!("Duplicate deposit detected: {}", deposit_id);
            return;
        }

        self.available += amount;
        self.processed_deposit_ids.insert(deposit_id);
    }
}
```

## Withdrawal Flow (UBSCore → Blockchain)

```
1. User Request: "Withdraw 1 BTC"

2. Gateway → UBSCore: "Lock 1 BTC for withdrawal"

3. UBSCore:
   - Check: available >= 1.0? YES
   - Action: available -= 1.0, frozen_for_withdrawal += 1.0
   - Emit: WithdrawalApproved { user, 1 BTC }

4. Payment Gateway (Cold):
   - Consume WithdrawalApproved
   - Broadcast BTC transaction

5. Blockchain: Confirms transaction

6. Payment Gateway:
   - Emit: WithdrawalFinalized { user, 1 BTC }

7. UBSCore:
   - Consume WithdrawalFinalized
   - Action: frozen_for_withdrawal -= 1.0 (burns it)
```

## UBSCore Crate Structure

```rust
// crate: ubs_core

use std::collections::HashMap;

/// The generic high-performance engine
pub struct UBSCore<T: RiskModel> {
    // 1. The Hot Wallet (The Money)
    // Sharded by UserID
    accounts: HashMap<UserId, Account>,

    // 2. The Logic (The Rules)
    // Trait allows swapping Spot logic vs Futures logic
    risk_model: T,

    // 3. The Output (The Nervous System)
    // Aeron Publisher to Matching Engine
    me_publisher: AeronPublication,

    // 4. Event Log (For persistence)
    event_producer: KafkaProducer,
}

impl<T: RiskModel> UBSCore<T> {
    pub fn on_order_request(&mut self, order: Order) {
        // Step 1: Check Balance (In RAM)
        let account = match self.accounts.get_mut(&order.user_id) {
            Some(acc) => acc,
            None => return self.reject(order, "Account not found"),
        };

        // Step 2: Calculate Risk (Spot or Future)
        if !self.risk_model.can_trade(account, &order) {
            return self.reject(order, "Insufficient balance");
        }

        // Step 3: Mutate State (Lock funds)
        account.lock_funds(order.cost);

        // Step 4: Fire Signal (To Matching Engine via Aeron)
        self.me_publisher.offer(&order.to_sbe_bytes());
    }

    pub fn on_trade_executed(&mut self, trade: Trade) {
        // Hot Path: Speculative credit (Aeron, ~100ns)
        let buyer = self.accounts.get_mut(&trade.buyer_id).unwrap();
        buyer.apply_speculative_credit(trade.base_asset, trade.quantity);

        // Cold Path: Persist to Kafka (async)
        self.event_producer.send(&trade);
    }
}

/// Trait for different market types
pub trait RiskModel {
    fn can_trade(&self, account: &Account, order: &Order) -> bool;
    fn calculate_margin(&self, account: &Account, position: &Position) -> u64;
}

/// Spot is simple
pub struct SpotRiskModel;

impl RiskModel for SpotRiskModel {
    fn can_trade(&self, account: &Account, order: &Order) -> bool {
        account.available() >= order.cost
    }

    fn calculate_margin(&self, _account: &Account, _position: &Position) -> u64 {
        0 // Spot has no margin
    }
}

/// Futures is complex
pub struct FuturesRiskModel {
    mark_price_feed: MarkPriceFeed,
}

impl RiskModel for FuturesRiskModel {
    fn can_trade(&self, account: &Account, order: &Order) -> bool {
        let margin_required = self.calculate_initial_margin(order);
        account.available() >= margin_required
    }

    fn calculate_margin(&self, account: &Account, position: &Position) -> u64 {
        // Complex calculation: mark price, PnL, funding rate, etc.
        let mark_price = self.mark_price_feed.get(position.symbol_id);
        let unrealized_pnl = position.calculate_pnl(mark_price);
        // ... more math
        0
    }
}
```

## Routing Rules

| Order Type | Route To | Reason |
|------------|----------|--------|
| Spot Order | UBSCore_Spot | Uses Spot balance |
| Futures Order | UBSCore_Futures | Uses Futures balance |
| Transfer (Spot→Future) | UBSCore_Spot first | Source decrements first |
| Transfer (Future→Spot) | UBSCore_Futures first | Source decrements first |
| Deposit | Correct UBSCore cluster | Based on asset type |
| Withdrawal | Correct UBSCore cluster | Based on asset type |

**Critical**: Gateway is STATELESS. Only UBSCore can modify balances.

## Fake "Unified Margin" (Auto-Sweeper)

To give users the UX of unified margin without the complexity:

```rust
/// Auto-Sweeper Bot
/// Runs in the background, monitors liquidation risk
struct AutoSweeper {
    spot_client: UBSCoreClient,
    futures_client: UBSCoreClient,
}

impl AutoSweeper {
    async fn monitor_liquidation_risk(&self) {
        loop {
            for user_id in self.get_unified_margin_users() {
                let futures_health = self.futures_client.get_margin_health(user_id);

                if futures_health < LIQUIDATION_THRESHOLD {
                    // Check if user has spot balance
                    let spot_balance = self.spot_client.get_available(user_id);

                    if spot_balance > 0 {
                        // Emergency transfer!
                        let amount = min(spot_balance, futures_health.deficit);
                        self.trigger_transfer(user_id, Spot, Futures, amount).await;

                        log::info!("Auto-swept {} from Spot to Futures for user {}",
                            amount, user_id);
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
```

## Summary

| Component | Name | Role |
|-----------|------|------|
| Gateway | Gateway | Dumb router, auth, JSON→Binary |
| Risk Engine | **UBSCore** | In-memory balance authority |
| Matching Engine | ME | Order book matching |
| Settlement | **Offshore** | Cold database, blockchain |

| Cluster | Purpose |
|---------|---------|
| UBSCore_Spot | Spot market balances |
| UBSCore_Futures | Futures market balances |
| ME_Spot_* | Spot order books |
| ME_Futures_* | Futures order books |

**The mental model**: UBSCore is the "Central Bank" of your exchange. The Matching Engines are "Merchants" asking the Central Bank if a transaction is valid.
