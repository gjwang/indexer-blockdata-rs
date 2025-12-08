# Transport Layer: Aeron vs ZeroMQ

## Summary: When to Use What

| Use Case | Technology | Latency | Reason |
|----------|------------|---------|--------|
| **Hot Path** (ME → Risk) | Aeron IPC/UDP | < 20 µs | Zero-copy, no TCP blocking |
| **Cold Path** (ME → Settlement) | Redpanda/Kafka | 2-10 ms | Persistence, replay |
| Client Gateway | ZeroMQ | 150-500 µs | Simple, robust TCP |
| Dashboard/Logging | ZeroMQ | ms range | Good enough |

## The Fundamental Difference: TCP vs Reliable UDP

### ZeroMQ (TCP-based)

```
Problem: Head-of-Line Blocking

Packet 1 → Received ✓
Packet 2 → LOST ✗
Packet 3 → Waiting... (blocked by packet 2!)
Packet 4 → Waiting...
Packet 5 → Waiting...

Kernel retransmits packet 2... (takes 50ms)

Result: Latency spike from 50µs to 50ms = DEATH in HFT
```

### Aeron (Reliable UDP)

```
Solution: No Head-of-Line Blocking

Packet 1 → Received ✓
Packet 2 → LOST ✗ (NAK sent, will be retransmitted)
Packet 3 → Received ✓ (not blocked!)
Packet 4 → Received ✓
Packet 5 → Received ✓
Packet 2 → Retransmitted, received ✓

Result: Stream continues flowing, one lost packet doesn't block everything
```

## Performance Comparison

| Feature | ZeroMQ (TCP) | Aeron (UDP/IPC) |
|---------|--------------|-----------------|
| **Transport** | TCP (mostly), IPC, PGM | Reliable UDP, Shared Memory |
| **Latency (p99)** | 150 - 500 µs | < 20 µs |
| **Latency (IPC)** | 10-50 µs (Unix socket) | **~100 ns** (Shared Memory) |
| **Throughput** | 100k - 500k msgs/sec | > 10M msgs/sec |
| **Garbage** | Some allocation | Zero-allocation |
| **Multicast** | PGM (fragile) | First-class (robust) |
| **Complexity** | Simple | Higher |

## Why Aeron Wins for Hot Path

### Reason 1: Shared Memory IPC

If ME and Risk Engine are on the same machine:

```
ZeroMQ IPC:
  ME → Unix Domain Socket → Kernel → Risk Engine
  Latency: 10-50 µs (kernel context switches)

Aeron IPC:
  ME → mmap Shared Memory File → Risk Engine
  The kernel is NOT involved in data transfer!
  Latency: ~100 nanoseconds
```

### Reason 2: Media Driver Isolation

```
ZeroMQ:
  ┌─────────────────────────────────┐
  │      Matching Engine Process    │
  │                                 │
  │  [App Logic] ←→ [ZMQ I/O Thread]│
  │                                 │
  │  If App does GC or CPU spike,   │
  │  network handling suffers       │
  └─────────────────────────────────┘

Aeron:
  ┌─────────────────────────────────┐  ┌─────────────────────┐
  │      Matching Engine Process    │  │    Media Driver     │
  │                                 │  │   (Separate Process)│
  │  [App Logic] ←→ [Shared Mem] ←─┼──┼→ [Network NIC]      │
  │                                 │  │                     │
  │  App can spike, media driver    │  │  Always responsive  │
  │  continues receiving packets    │  │                     │
  └─────────────────────────────────┘  └─────────────────────┘
```

The Media Driver is a separate process that handles:
- Packet sending/receiving
- Flow control
- NAK/retransmission

Your application logic never blocks network I/O.

### Reason 3: Zero-Copy Design

```
ZeroMQ:
  Network Buffer → Copy → ZMQ Buffer → Copy → Application Buffer

Aeron:
  Network Buffer → (via Media Driver) → Shared Memory → Application Reads Directly
  No copies in the hot path!
```

## Aeron Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                        MACHINE 1                                │
│                                                                 │
│  ┌─────────────────┐    Shared Memory     ┌─────────────────┐  │
│  │ Matching Engine │◄──────────────────►│   Risk Engine   │  │
│  │                 │    (Aeron IPC)       │                 │  │
│  │  Publisher      │    ~100 ns latency   │   Subscriber    │  │
│  └────────┬────────┘                      └─────────────────┘  │
│           │                                                     │
│           │ Publications go through                             │
│           ▼                                                     │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                    MEDIA DRIVER                          │   │
│  │                                                          │   │
│  │  Manages all Aeron channels                              │   │
│  │  Handles UDP send/receive                                │   │
│  │  Flow control, NAKs, retransmission                      │   │
│  │                                                          │   │
│  │  Can run as:                                             │   │
│  │  - Embedded (in-process, simpler)                        │   │
│  │  - Standalone (separate process, more isolation)         │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

## Aeron for Different Deployments

### Same Machine (IPC)
```
ME → Aeron IPC (shared memory) → Risk Engine
Latency: ~100 ns
Use: aeron:ipc channel
```

### Different Machines (UDP)
```
Machine A (ME) → Aeron UDP → Machine B (Risk Engine)
Latency: 5-20 µs
Use: aeron:udp?endpoint=192.168.1.2:40123
```

### Multicast (Market Data Fan-out)
```
ME → Aeron Multicast → [Subscriber 1, Subscriber 2, ...]
Use: aeron:udp?endpoint=224.0.1.1:40456|interface=192.168.1.1
```

## Rust Implementation with Aeron

### Publisher (Matching Engine)
```rust
use aeron_rs::aeron::Aeron;
use aeron_rs::concurrent::atomic_buffer::AtomicBuffer;
use aeron_rs::publication::Publication;

struct HotPathPublisher {
    publication: Publication,
}

impl HotPathPublisher {
    fn new() -> Self {
        let aeron = Aeron::new_with_default_context().unwrap();

        // IPC channel for same-machine communication
        let channel = "aeron:ipc";
        let stream_id = 1001;

        let publication = aeron.add_publication(channel, stream_id).unwrap();

        HotPathPublisher { publication }
    }

    fn send_fill_event(&self, event: &FastFillEvent) {
        let bytes = bincode::serialize(event).unwrap();

        // Try to publish (non-blocking)
        loop {
            let result = self.publication.offer(&bytes);
            match result {
                Ok(position) => {
                    // Successfully published
                    break;
                }
                Err(e) if e.is_back_pressured() => {
                    // Subscriber is slow, spin
                    std::hint::spin_loop();
                }
                Err(e) => {
                    log::error!("Aeron publish failed: {:?}", e);
                    break;
                }
            }
        }
    }
}
```

### Subscriber (Risk Engine)
```rust
use aeron_rs::aeron::Aeron;
use aeron_rs::subscription::Subscription;
use aeron_rs::concurrent::logbuffer::header::Header;

struct HotPathSubscriber {
    subscription: Subscription,
}

impl HotPathSubscriber {
    fn new() -> Self {
        let aeron = Aeron::new_with_default_context().unwrap();

        let channel = "aeron:ipc";
        let stream_id = 1001;

        let subscription = aeron.add_subscription(channel, stream_id).unwrap();

        HotPathSubscriber { subscription }
    }

    fn poll(&self) -> Option<FastFillEvent> {
        let mut result = None;

        // Poll for fragments (non-blocking)
        self.subscription.poll(|buffer: &[u8], _header: &Header| {
            let event: FastFillEvent = bincode::deserialize(buffer).unwrap();
            result = Some(event);
        }, 1);

        result
    }

    fn run_loop(&self, risk_engine: &mut RiskEngine) {
        loop {
            // Tight polling loop (LMAX-style)
            if let Some(event) = self.poll() {
                risk_engine.handle_hot_path(event);
            } else {
                // No message, maybe yield or spin
                std::hint::spin_loop();
            }
        }
    }
}
```

## Media Driver Configuration

### Embedded Driver (Simpler)
```rust
// Driver runs inside your process
let context = Context::new();
context.set_dir_name("/dev/shm/aeron-risk-engine");

// This starts the media driver in a background thread
let aeron = Aeron::new(context).unwrap();
```

### Standalone Driver (Production)
```bash
# Start media driver as separate process
$ java -cp aeron-all.jar io.aeron.driver.MediaDriver \
    -Daeron.dir=/dev/shm/aeron-fast-path \
    -Daeron.threading.mode=DEDICATED \
    -Daeron.sender.idle.strategy=noop \
    -Daeron.receiver.idle.strategy=noop
```

For maximum performance:
- DEDICATED threading mode (separate threads for conductor, sender, receiver)
- NOOP idle strategy (busy spin, uses more CPU but lowest latency)
- Use `/dev/shm` for shared memory (RAM-backed filesystem)

## When to Use What

### Use Aeron for:
- **ME → Risk Engine** (Hot Path, capital recycling)
- **Market Data Fan-out** (Multicast to many subscribers)
- **Internal Low-Latency Messaging** (between core components)

### Use ZeroMQ for:
- **Client Connections** (TCP is fine for external clients)
- **Logging/Monitoring** (not latency critical)
- **Dashboard Data** (not latency critical)
- **Simpler services** where µs don't matter

### Use Kafka/Redpanda for:
- **Cold Path** (ME → Settlement)
- **Event Sourcing** (replayable log)
- **Audit Trail** (persistent, ordered)

## Complete Hot Path Stack

```
┌─────────────────────────────────────────────────────────────────────┐
│                     MATCHING ENGINE                                   │
│                                                                       │
│   Trade Executed                                                      │
│        │                                                              │
│        ├────────────────────────────────────┐                        │
│        │                                    │                        │
│        ▼                                    ▼                        │
│   ┌─────────────────────┐         ┌─────────────────────┐           │
│   │   Aeron Publisher   │         │   Kafka Producer    │           │
│   │   (Hot Path)        │         │   (Cold Path)       │           │
│   │   aeron:ipc         │         │   TCP to broker     │           │
│   └──────────┬──────────┘         └──────────┬──────────┘           │
│              │                               │                       │
└──────────────┼───────────────────────────────┼───────────────────────┘
               │                               │
               │ ~100 ns                       │ 2-10 ms
               ▼                               ▼
┌──────────────────────────┐        ┌─────────────────────┐
│      RISK ENGINE         │        │     REDPANDA        │
│                          │        │                     │
│  Aeron Subscriber        │        │  Persisted Log      │
│  Speculative Credit      │        │  Offset-based order │
│  Capital Recycling       │        │                     │
│                          │        └──────────┬──────────┘
│  Latency: 100 ns         │                   │
└──────────────────────────┘                   │
                                               │
                                               ▼
                                  ┌─────────────────────┐
                                  │  SETTLEMENT SERVICE │
                                  │                     │
                                  │  Confirms Hot Path  │
                                  │  Dirty → Clean      │
                                  │  Writes to DB       │
                                  │                     │
                                  │  Latency: 2-10 ms   │
                                  └─────────────────────┘
```

## Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Hot Path Transport | **Aeron IPC** | 100 ns latency, zero-copy, no TCP blocking |
| Cold Path Transport | **Redpanda** | Persistence, replay, deterministic order |
| Client Gateway | **ZeroMQ/TCP** | Simpler, robust for external clients |
| Market Data Fan-out | **Aeron Multicast** | Low-latency one-to-many |

**Don't let Aeron's complexity scare you.** For an exchange, it's the price of admission for the performance you need.
