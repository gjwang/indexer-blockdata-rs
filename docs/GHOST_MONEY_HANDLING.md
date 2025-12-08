# Ghost Money: Handling Speculative Funds Failure

## The Nightmare Scenario

```
User A has 0 USDT

T+0:    BTC-ME executes: User A sells 0.1 BTC → +5000 USDT (Dirty)
T+10µs: Risk Engine receives Hot Path → Balance: 5000 USDT (Dirty)
T+20µs: User A places order: Buy 0.1 ETH with 5000 USDT
T+30µs: ETH-ME executes → User A has 0.1 ETH, User B expects 5000 USDT

T+50µs: BTC-ME CRASHES before writing to Redpanda 💀

Result:
- User A has 0.1 ETH they didn't pay for
- User B sold ETH expecting real USDT
- Exchange is missing 5000 USDT
- The BTC sale "vanished" (Ghost Money)
```

## Why You Can't Just "Cancel" the ETH Trade

```
User B (ETH seller) might have already:
  → Used the 5000 USDT to buy XRP
  → User C sold XRP expecting that USDT
  → User C used USDT to buy DOGE
  → ...

Cascading rollback = Market chaos
```

**Once a trade is in the Log, it's FINAL.**

## Solution 1: The Debt Ledger (Separate from Balance)

### 🚨 Critical Design Decision

**Balance uses `u64` (never negative) + Separate `DebtLedger` for ghost money.**

**Why NOT i64 for Balance?**
- Balance stays simple, fast, robust
- u64 is enforced by compiler (can't accidentally go negative)
- Debt is rare - separate handling is cleaner
- Full audit trail for debts

### Implementation

```rust
/// Balance stays pure u64 - NEVER negative
pub struct Balance {
    pub avail: u64,
    pub frozen: u64,
    pub version: u64,
}

/// Separate ledger for debts (ghost money scenarios)
pub struct DebtLedger {
    debts: HashMap<(UserId, AssetId), DebtRecord>,
}

pub struct DebtRecord {
    pub amount: u64,          // Debt amount (always positive)
    pub created_at: u64,      // Timestamp
    pub reason: DebtReason,   // Why debt occurred
    pub sequence: u64,        // WAL sequence for audit
}

pub enum DebtReason {
    GhostMoney,         // Trade settled without funds
    Liquidation,        // Forced position close deficit
    FeeUnpaid,          // Fee charged but no balance
    StaleSpeculative,   // Speculative credit timed out
}
```

### How It Works

```
Scenario: User has 0 USDT, must deduct 5000 USDT

Old approach (i64):
  balance.avail = 0 - 5000 = -5000  // Negative in Balance

New approach (u64 + DebtLedger):
  balance.avail = 0                 // Stays 0 (saturating_sub)
  debt_ledger.add(user_id, USDT, DebtRecord {
      amount: 5000,
      reason: DebtReason::GhostMoney,
  })
```

### Withdrawal Firewall with Debt

```rust
impl UBSCore {
    pub fn can_withdraw(&self, user_id: UserId, asset_id: AssetId, amount: u64) -> bool {
        // CANNOT withdraw if user has ANY debt
        if self.debt_ledger.has_debt(user_id) {
            return false;
        }

        let balance = self.get_balance(user_id, asset_id);
        balance.avail >= amount
    }

    pub fn on_deposit(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) {
        // First: Clear debt for this asset
        let mut remaining = amount;

        if let Some(debt) = self.debt_ledger.get_mut(user_id, asset_id) {
            let pay_off = remaining.min(debt.amount);
            debt.amount -= pay_off;
            remaining -= pay_off;

            if debt.amount == 0 {
                self.debt_ledger.remove(user_id, asset_id);
            }
        }

        // Then: Deposit remaining to Balance
        if remaining > 0 {
            let balance = self.get_balance_mut(user_id, asset_id);
            balance.deposit(remaining);
        }
    }
}
```

### Debt Audit Trail

```rust
impl DebtLedger {
    pub fn add_debt(&mut self, user_id: UserId, asset_id: AssetId, record: DebtRecord) {
        // Log for audit
        log::error!(
            "DEBT_CREATED: user={} asset={} amount={} reason={:?} seq={}",
            user_id, asset_id, record.amount, record.reason, record.sequence
        );

        // Insert or update
        self.debts
            .entry((user_id, asset_id))
            .and_modify(|existing| existing.amount += record.amount)
            .or_insert(record);
    }

    pub fn total_debt(&self, user_id: UserId) -> u64 {
        self.debts.iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, r)| r.amount)
            .sum()
    }
}
```

**Benefits of DebtLedger**:
- Balance stays simple `u64` - fast, robust, no edge cases
- Debt is explicit and auditable
- Full history of why debt occurred
- Deposits auto-pay debt first

## Solution 2: Auto-Liquidation Protocol

```rust
struct RiskEngine {
    balances: HashMap<(UserId, AssetId), Balance>,
}

impl RiskEngine {
    /// Called during reconciliation or periodic check
    fn check_negative_balances(&mut self) {
        for ((user_id, asset_id), balance) in &self.balances {
            if balance.total() < 0 {
                self.trigger_liquidation(*user_id, *asset_id, balance.total());
            }
        }
    }

    fn trigger_liquidation(&mut self, user_id: UserId, debt_asset: AssetId, debt: i64) {
        log::error!("LIQUIDATION: user={} owes {} of asset {}",
            user_id, debt.abs(), debt_asset);

        // 1. Lock account (no new orders, no withdrawals)
        self.lock_account(user_id);

        // 2. Find assets to sell
        let sellable_assets = self.find_sellable_assets(user_id);

        // 3. Inject market sell orders
        for (asset_id, amount) in sellable_assets {
            // Calculate how much to sell to cover debt
            let sell_order = Order {
                user_id,
                side: Side::Sell,
                order_type: OrderType::Market,
                symbol_id: get_trading_pair(asset_id, debt_asset),
                quantity: amount,
                // This is a SYSTEM order, bypasses balance check
                is_liquidation: true,
            };

            self.inject_liquidation_order(sell_order);
        }

        // 4. After liquidation settles, check remaining debt
        // Insurance fund covers any remaining loss
    }
}
```

### Liquidation Flow

```
Detection: Risk Engine sees USDT_Balance = -5000

Step 1: Lock Account
  → User can't withdraw
  → User can't place new orders
  → Existing orders canceled

Step 2: Forced Liquidation
  → System injects: SELL 0.1 ETH @ MARKET
  → Trade executes: +4950 USDT (slippage)

Step 3: Settlement
  → Balance: -5000 + 4950 = -50 USDT

Step 4: Insurance Fund
  → Exchange covers the -50 USDT
  → User balance: 0 USDT
  → Account unlocked (maybe with warning)
```

## Solution 3: Prevent Ghost Money (Local WAL)

**The ME must write to local disk BEFORE sending Hot Path UDP.**

### Bad Flow (Ghost Money Risk)
```
Match → Send UDP → Write to Disk
              ↓
        CRASH HERE = Trade vanishes
```

### Good Flow (Safe)
```
Match → Write to Local WAL → Send UDP
                    ↓
              CRASH HERE = Trade still on disk
```

```rust
impl MatchingEngine {
    fn execute_trade(&mut self, trade: Trade) {
        // Step 1: Write to LOCAL WAL FIRST (NVMe, fast)
        self.wal.append(&trade)?;
        self.wal.sync()?;  // fsync - data is on disk

        // Step 2: NOW safe to send Hot Path
        self.send_hot_path(&trade);

        // Step 3: Async send to Redpanda (Cold Path)
        self.send_cold_path(&trade);
    }
}
```

### Recovery After Crash

```
ME restarts:
1. Read local WAL from last checkpoint
2. Find trades NOT in Redpanda (missed Cold Path)
3. Re-publish to Redpanda
4. The "Dirty" funds were real - just late!
5. Math balances out
```

## Solution 4: Withdrawal Firewall

**Hard Rule: You can TRADE with dirty funds, but CANNOT WITHDRAW them.**

```rust
struct Balance {
    // Confirmed by Cold Path (Redpanda)
    confirmed: u64,

    // Speculative from Hot Path
    speculative: Vec<SpeculativeCredit>,

    // Locked for pending orders
    locked: u64,
}

impl Balance {
    /// For TRADING - includes speculative
    fn tradeable(&self) -> u64 {
        self.confirmed + self.speculative_total() - self.locked
    }

    /// For WITHDRAWAL - only confirmed funds
    fn withdrawable(&self) -> u64 {
        self.confirmed.saturating_sub(self.locked)
    }
}
```

### Why This Matters

```
Trading: Internal database update
  → Fast
  → Reversible via liquidation
  → Mess stays inside exchange

Withdrawal: Blockchain transaction
  → Slow (confirmation time)
  → IRREVERSIBLE
  → If wrong, money is GONE
```

**The worst case (Ghost Money) stays inside the exchange where you can fix it.**

## Complete Risk Engine Implementation

```rust
struct RiskEngine {
    balances: HashMap<(UserId, AssetId), Balance>,
    account_status: HashMap<UserId, AccountStatus>,
}

#[derive(PartialEq)]
enum AccountStatus {
    Active,
    Locked,        // Can't trade or withdraw
    Liquidating,   // Forced sells in progress
}

struct Balance {
    confirmed: i64,               // Can be negative (debt)!
    speculative: Vec<SpeculativeCredit>,
    locked: u64,
}

impl RiskEngine {
    /// Check if order can be placed
    fn can_place_order(&self, order: &Order) -> bool {
        // Account must be active
        if self.account_status.get(&order.user_id) != Some(&AccountStatus::Active) {
            return false;
        }

        // Check tradeable balance (includes speculative)
        let balance = self.balances.get(&(order.user_id, order.lock_asset));
        balance.tradeable() >= order.lock_amount
    }

    /// Check if withdrawal can proceed
    fn can_withdraw(&self, user_id: UserId, asset_id: AssetId, amount: u64) -> bool {
        // Account must be active
        if self.account_status.get(&user_id) != Some(&AccountStatus::Active) {
            return false;
        }

        // Check WITHDRAWABLE balance (confirmed only, no speculative)
        let balance = self.balances.get(&(user_id, asset_id));
        balance.withdrawable() >= amount
    }

    /// Handle Hot Path (speculative credit)
    fn handle_hot_path(&mut self, event: &FastFillEvent) {
        let balance = self.balances.entry((event.user_id, event.asset_id))
            .or_insert_with(Balance::default);

        balance.speculative.push(SpeculativeCredit {
            event_id: event.event_id,
            amount: event.amount,
            timestamp_ns: now_ns(),
        });
    }

    /// Handle Cold Path (confirmation)
    fn handle_cold_path(&mut self, event: &SettlementEvent) {
        let balance = self.balances.entry((event.user_id, event.asset_id))
            .or_insert_with(Balance::default);

        // Try to match with speculative
        if let Some(idx) = balance.speculative.iter()
            .position(|s| s.event_id == event.event_id)
        {
            // Match! Convert speculative → confirmed
            let credit = balance.speculative.remove(idx);
            balance.confirmed += credit.amount as i64;
        } else {
            // No matching speculative (maybe Hot Path was lost)
            // Just add to confirmed directly
            balance.confirmed += event.amount as i64;
        }

        // Check for negative balance (Ghost Money scenario)
        if balance.confirmed < 0 {
            self.trigger_liquidation(event.user_id);
        }
    }

    /// Handle missing Hot Path (Cold Path came but no speculative)
    fn handle_orphan_cold_path(&mut self, event: &SettlementEvent) {
        log::warn!("Orphan Cold Path: event_id={}", event.event_id);

        // This means the Hot Path UDP was lost
        // The Cold Path is the truth, apply it
        let balance = self.balances.entry((event.user_id, event.asset_id))
            .or_insert_with(Balance::default);

        balance.confirmed += event.amount as i64;

        // User might have already used this money (via another Hot Path)
        // Check for negative balance
        if balance.confirmed < 0 {
            self.trigger_liquidation(event.user_id);
        }
    }

    /// Periodic cleanup of stale speculative credits
    fn reconcile_speculative(&mut self) {
        let timeout = Duration::from_secs(30);
        let now = now_ns();

        for ((user_id, asset_id), balance) in &mut self.balances {
            let stale: Vec<_> = balance.speculative.iter()
                .filter(|s| Duration::from_nanos(now - s.timestamp_ns) > timeout)
                .collect();

            for credit in &stale {
                log::error!(
                    "STALE SPECULATIVE: user={} asset={} amount={} event_id={}",
                    user_id, asset_id, credit.amount, credit.event_id
                );
            }

            // Remove stale credits (they were Ghost Money)
            balance.speculative.retain(|s|
                Duration::from_nanos(now - s.timestamp_ns) <= timeout
            );

            // Removing speculative might cause negative balance
            if balance.total() < 0 {
                self.trigger_liquidation(*user_id);
            }
        }
    }
}
```

## Summary: Ghost Money Handling

| Scenario | Detection | Action |
|----------|-----------|--------|
| Hot Path lost, Cold Path arrives | Normal flow | Apply Cold Path directly |
| Cold Path lost (ME crash before disk) | Stale speculative (30s) | Revoke + potentially liquidate |
| Both lost (catastrophic) | Reconciliation with DB | Force correct, liquidate if needed |
| User spent ghost money | Negative balance | Auto-liquidation + insurance |

## Safety Layers

```
Layer 1: Local WAL
  → ME writes to disk BEFORE Hot Path
  → Minimizes Ghost Money window

Layer 2: Speculative Timeout
  → 30s without Cold Path confirmation = revoke
  → Limits exposure

Layer 3: Withdrawal Firewall
  → Can't withdraw speculative funds
  → Worst case stays inside exchange

Layer 4: Negative Balance Detection
  → Immediate account lock
  → Auto-liquidation

Layer 5: Insurance Fund
  → Covers remaining debt after liquidation
  → Exchange absorbs tiny loss vs slow everyone down
```

**The entire system is designed to contain the blast radius of Ghost Money within the exchange, where it can be fixed, rather than letting it escape to the blockchain.**
