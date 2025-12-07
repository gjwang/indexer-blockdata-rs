# Speculative Execution: Hot Path / Cold Path Architecture

## The Trade-Off: Latency vs Consistency

| Path | Latency | Consistency | When to Use |
|------|---------|-------------|-------------|
| Cold Path (Redpanda only) | 2-10ms | Strong | Account statements |
| Hot Path (Speculative) | 20-100µs | Eventual | Capital recycling |

**Capital Recycling**: User sells BTC, immediately uses USDT to buy ETH.
- Cold path: Wait 2-10ms for Redpanda confirmation
- Hot path: Available in **20µs**

## Architecture: Dual-Lane Messaging

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        MATCHING ENGINE                                   │
│                                                                          │
│   Trade Executed: User_A sells 0.1 BTC for 5000 USDT                    │
│                                                                          │
│   FIRE TWO MESSAGES SIMULTANEOUSLY:                                      │
│                                                                          │
│   ┌─────────────────┐        ┌─────────────────┐                        │
│   │  HOT PATH (A)   │        │  COLD PATH (B)  │                        │
│   │  UDP/Aeron/SHM  │        │  Redpanda/Kafka │                        │
│   │  20-100 µs      │        │  2-10 ms        │                        │
│   └────────┬────────┘        └────────┬────────┘                        │
└────────────┼─────────────────────────┼──────────────────────────────────┘
             │                         │
             │ Arrives FIRST           │ Arrives SECOND
             │ (Speculative)           │ (Final Truth)
             ▼                         ▼
┌────────────────────────────┐  ┌────────────────────────────┐
│       RISK ENGINE          │  │        REDPANDA            │
│                            │  │                            │
│  ┌──────────────────────┐  │  │   Offset 101: Trade_A      │
│  │ Balance (Speculative)│  │  │                            │
│  │                      │  │  └────────────┬───────────────┘
│  │ USDT: 5000 (DIRTY)   │  │               │
│  │ Tag: "Unsettled"     │  │               │ Consumed later
│  │ Event_ID: 12345      │  │               │
│  └──────────────────────┘  │               ▼
│                            │  ┌────────────────────────────┐
│  User can trade NOW!       │  │   SETTLEMENT SERVICE       │
│  Buy ETH with DIRTY USDT   │  │                            │
│                            │  │   Confirms Event_ID 12345  │
│                            │◀─┤   Risk Engine reconciles   │
│                            │  │   DIRTY → CLEAN            │
└────────────────────────────┘  └────────────────────────────┘
```

## Risk Engine: Speculative State

```rust
struct UserBalance {
    asset_id: u32,

    // Clean balance (confirmed by Redpanda)
    confirmed: u64,

    // Speculative additions (Hot Path, not yet confirmed)
    speculative: Vec<SpeculativeCredit>,

    // Locked for pending orders
    locked: u64,
}

struct SpeculativeCredit {
    event_id: u64,      // To match with Cold Path
    amount: u64,
    timestamp_ns: u64,  // For timeout/cleanup
}

impl UserBalance {
    fn available(&self) -> u64 {
        // User can spend confirmed + speculative - locked
        self.confirmed + self.speculative_total() - self.locked
    }

    fn speculative_total(&self) -> u64 {
        self.speculative.iter().map(|s| s.amount).sum()
    }
}
```

## Message Flow: Step by Step

### 1. Trade Executes
```
ME executes: User_A sells 0.1 BTC for 5000 USDT

ME fires simultaneously:
- Hot Path: UDP packet → Risk Engine
- Cold Path: Kafka message → Redpanda
```

### 2. Hot Path Arrives (20-100 µs)
```rust
// Risk Engine receives UDP packet
fn handle_hot_path(&mut self, event: FastFillEvent) {
    let balance = self.balances.get_mut(&(event.user_id, event.asset_id));

    // Add as SPECULATIVE (not confirmed yet)
    balance.speculative.push(SpeculativeCredit {
        event_id: event.event_id,
        amount: event.amount,
        timestamp_ns: now(),
    });

    // Log for debugging
    log::debug!("Speculative credit: user={} asset={} amount={} event_id={}",
        event.user_id, event.asset_id, event.amount, event.event_id);
}
```

### 3. User Places New Order (Immediately!)
```rust
// User wants to buy ETH with the USDT they just received
fn handle_order(&mut self, order: Order) {
    let balance = self.balances.get(&(order.user_id, order.quote_asset));

    // available() includes speculative funds!
    if balance.available() >= order.amount {
        // Lock funds (can use speculative!)
        self.lock_funds(order.user_id, order.quote_asset, order.amount);

        // Forward to ME
        self.forward_to_me(order);
    } else {
        self.reject_order(order, "Insufficient balance");
    }
}
```

### 4. Cold Path Arrives (2-10 ms later)
```rust
// Settlement service consumes from Redpanda, notifies Risk Engine
fn handle_cold_path(&mut self, event: SettlementEvent) {
    let balance = self.balances.get_mut(&(event.user_id, event.asset_id));

    // Find matching speculative credit
    if let Some(idx) = balance.speculative.iter()
        .position(|s| s.event_id == event.event_id)
    {
        // MATCH! Convert speculative → confirmed
        let credit = balance.speculative.remove(idx);
        balance.confirmed += credit.amount;

        log::debug!("Confirmed credit: user={} asset={} amount={} event_id={}",
            event.user_id, event.asset_id, credit.amount, event.event_id);
    } else {
        // Cold path event without matching hot path
        // Maybe hot path UDP was dropped?
        // Apply directly to confirmed (safe, just slower)
        balance.confirmed += event.amount;

        log::warn!("Cold path without hot path: event_id={}", event.event_id);
    }
}
```

### 5. Reconciliation (Self-Healing)
```rust
// Periodic check for stale speculative credits
fn reconcile(&mut self) {
    let timeout = Duration::from_secs(30); // 30s is VERY generous
    let now = now_ns();

    for (key, balance) in &mut self.balances {
        // Remove stale speculative credits
        balance.speculative.retain(|credit| {
            let age = Duration::from_nanos(now - credit.timestamp_ns);
            if age > timeout {
                // This should NEVER happen if system is healthy
                log::error!("ALERT: Stale speculative credit! event_id={} age={:?}",
                    credit.event_id, age);

                // DO NOT add to confirmed - wait for cold path
                // Trigger investigation
                false
            } else {
                true
            }
        });
    }
}
```

## Technology Stack

### Option 1: Aeron (Industry Standard)
```
Aeron: High-performance messaging transport (UDP-based)
- Used by HFT firms (LMAX, etc.)
- Reliable multicast/unicast over UDP
- Latency: < 20 µs round trip
- Has its own log (Term Buffers) for replay
```

### Option 2: Unix Domain Socket (Same Machine)
```
If ME and Risk Engine on same server:
- Unix Domain Socket: < 10 µs
- No network stack overhead
- Simple to implement in Rust
```

### Option 3: Shared Memory (Fastest)
```
Co-located ME + Risk Engine:
- Shared Memory (mmap): < 1 µs
- Zero-copy messaging
- Ring buffer pattern (LMAX Disruptor)
```

### Rust Implementation: Unix Domain Socket
```rust
// Hot Path: ME sends to Risk Engine via Unix socket
use tokio::net::UnixDatagram;

// ME side
async fn send_hot_path(socket: &UnixDatagram, event: &FastFillEvent) {
    let bytes = bincode::serialize(event).unwrap();
    socket.send(&bytes).await.unwrap();
    // Fire-and-forget, no wait
}

// Risk Engine side
async fn receive_hot_path(socket: &UnixDatagram) -> FastFillEvent {
    let mut buf = [0u8; 256];
    let n = socket.recv(&mut buf).await.unwrap();
    bincode::deserialize(&buf[..n]).unwrap()
}
```

### Rust Implementation: Shared Memory Ring Buffer
```rust
use std::sync::atomic::{AtomicU64, Ordering};
use memmap2::MmapMut;

struct RingBuffer {
    mmap: MmapMut,
    write_pos: AtomicU64,
    read_pos: AtomicU64,
    capacity: u64,
}

impl RingBuffer {
    // ME writes
    fn push(&self, event: &FastFillEvent) {
        let bytes = bincode::serialize(event).unwrap();
        let pos = self.write_pos.fetch_add(1, Ordering::SeqCst) % self.capacity;

        let offset = (pos * SLOT_SIZE) as usize;
        self.mmap[offset..offset + bytes.len()].copy_from_slice(&bytes);

        // Memory barrier to ensure visibility
        std::sync::atomic::fence(Ordering::Release);
    }

    // Risk Engine reads
    fn pop(&self) -> Option<FastFillEvent> {
        let write = self.write_pos.load(Ordering::Acquire);
        let read = self.read_pos.load(Ordering::Relaxed);

        if read >= write {
            return None; // Empty
        }

        let pos = read % self.capacity;
        let offset = (pos * SLOT_SIZE) as usize;

        let event: FastFillEvent = bincode::deserialize(&self.mmap[offset..]).unwrap();
        self.read_pos.fetch_add(1, Ordering::SeqCst);

        Some(event)
    }
}
```

## Latency Comparison

| Architecture | Trade → Capital Ready | Technology |
|--------------|----------------------|------------|
| Cold Path Only | 2-10 ms | Redpanda TCP |
| Hot Path (Aeron) | 20-50 µs | UDP |
| Hot Path (UDS) | 5-20 µs | Unix Domain Socket |
| Hot Path (SHM) | 0.1-1 µs | Shared Memory |

**100-200x faster** with Hot Path!

## Safety Guarantees

### The Log is Always Right
```
If Hot Path says: +5000 USDT
If Cold Path says: +4900 USDT (maybe fee was different?)

Result: Snap to Cold Path value
- Remove speculative credit
- Use Cold Path amount for confirmed
- Log discrepancy for investigation
```

### Rollback (Disaster Recovery)
```
If Hot Path gives credit but Cold Path never arrives:
1. Speculative credit ages out (30s timeout)
2. Credit is REVOKED
3. Any orders placed with that credit → force cancel
4. Alert operations team
```

### Idempotency
```
Both paths carry the same event_id.
Risk Engine only applies each event_id once.
Duplicate UDP packets → ignored.
Duplicate Kafka messages → ignored.
```

## Co-Location: The Cheat Code

```
┌────────────────────────────────────────────────┐
│              SAME PHYSICAL SERVER               │
│                                                 │
│  ┌─────────────┐      ┌─────────────────────┐  │
│  │     ME      │─SHM─▶│    Risk Engine      │  │
│  │             │      │                     │  │
│  │             │      │  Hot Path: ~100 ns  │  │
│  └──────┬──────┘      └─────────────────────┘  │
│         │                                       │
│         │ Kafka Produce (async)                 │
│         │                                       │
└─────────┼───────────────────────────────────────┘
          │
          ▼
┌─────────────────────┐
│      REDPANDA       │
│  (Cold Path, async) │
│                     │
│  For: Settlement    │
│       Audit Trail   │
│       Replay        │
└─────────────────────┘
```

**Risk Engine knows about trade in 100ns** via shared memory.
**Settlement still uses Redpanda** for durability and determinism.
**Best of both worlds!**

## Summary

| Aspect | Cold Path | Hot Path |
|--------|-----------|----------|
| **Latency** | 2-10 ms | 0.1-50 µs |
| **Reliability** | Guaranteed | Best-effort |
| **Durability** | Persisted | In-memory only |
| **Use case** | Settlement, Audit | Capital recycling |
| **Technology** | Kafka/Redpanda | Aeron/UDS/SHM |

**Golden Rule**: Hot Path speeds things up, Cold Path is the truth.
