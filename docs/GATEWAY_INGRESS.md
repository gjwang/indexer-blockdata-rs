# Gateway to Risk Engine: Order Entry Ingress

## Overview

The Gateway → Risk Engine link is the **Ingress** of your exchange. This defines **Order Entry Latency** - if this is slow, users perceive the whole exchange as slow.

## Responsibility Split

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GATEWAY                                         │
│                          "Human Stuff"                                       │
│                                                                              │
│   - WebSockets / REST / FIX                                                  │
│   - TLS termination                                                          │
│   - JSON parsing                                                             │
│   - API key validation                                                       │
│   - Rate limiting / DDoS protection                                          │
│   - Protocol normalization (JSON → Binary)                                   │
│                                                                              │
│   OUTPUT: Compact 64-byte binary packet                                      │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Binary only (SBE/Bincode)
                                    │ Routed by user_id % shard_count
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            RISK ENGINE                                       │
│                          "Machine Stuff"                                     │
│                                                                              │
│   - Binary struct processing ONLY                                            │
│   - RAM access for balance                                                   │
│   - No JSON, no TLS, no text parsing                                         │
│   - Pure number crunching                                                    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Routing: Sticky Connections (NOT Round-Robin)

**Problem**: User_A's balance lives ONLY in Risk_Node_4's RAM. If Gateway sends to Risk_Node_5, it will fail (State Miss).

**Solution**: Client-side sharding in Gateway code.

```rust
// Gateway Logic
struct GatewayRouter {
    risk_nodes: Vec<Connection>, // Persistent connections to all risk nodes
}

impl GatewayRouter {
    fn route_order(&self, order: &Order) -> &Connection {
        // Deterministic routing by user_id
        let shard_index = (order.user_id % self.risk_nodes.len() as u64) as usize;
        &self.risk_nodes[shard_index]
    }

    async fn handle_order(&self, json_request: &str) -> Result<(), Error> {
        // 1. Parse JSON (do this ONCE, here in Gateway)
        let order: OrderRequest = serde_json::from_str(json_request)?;

        // 2. Validate API key, signature, rate limits
        self.validate_auth(&order)?;

        // 3. Convert to binary (SBE or bincode)
        let binary_command = order.to_binary();

        // 4. Route to correct Risk shard
        let connection = self.route_order(&order);

        // 5. Send binary packet
        connection.send(&binary_command).await?;

        Ok(())
    }
}
```

## Connection Topology

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GATEWAY                                         │
│                                                                              │
│    [WebSocket Handler 1] ─┐                                                  │
│    [WebSocket Handler 2] ─┤                                                  │
│    [WebSocket Handler 3] ─┼─→ [Connection Pool] ─┐                          │
│    [WebSocket Handler 4] ─┤                      │                          │
│    [REST Handler 1]      ─┤                      │                          │
│    [REST Handler 2]      ─┘                      │                          │
│                                                   │                          │
└───────────────────────────────────────────────────┼──────────────────────────┘
                                                    │
                    Multiplexed over few persistent connections
                                                    │
        ┌───────────────────────────────────────────┼───────────────────────┐
        │                                           │                       │
        ▼                                           ▼                       ▼
┌───────────────┐                         ┌───────────────┐        ┌───────────────┐
│  Risk Node 0  │                         │  Risk Node 1  │        │  Risk Node 2  │
│  (user%3==0)  │                         │  (user%3==1)  │        │  (user%3==2)  │
└───────────────┘                         └───────────────┘        └───────────────┘
```

**Key**: Gateway maintains ONE persistent high-speed connection to EACH Risk shard, multiplexing thousands of user requests.

## Transport Options

### Option A: TCP / gRPC (Standard)

```
Pros:
- Easier to implement
- Well understood
- Good for retail exchanges

Cons:
- Higher latency (syscalls, buffering)
- 50-200 µs typical

Verdict: Acceptable for non-HFT retail exchanges
```

### Option B: Aeron UDP (Pro)

```
Pros:
- Consistent low latency (10-50 µs)
- No head-of-line blocking
- Fire-and-forget pattern

Verdict: Required for top-tier exchanges
```

## Command Queue Pattern (Fire and Forget)

Instead of request-response (RPC), use separate streams:

```
┌─────────────┐                              ┌─────────────┐
│   Gateway   │                              │ Risk Engine │
│             │                              │             │
│  ┌───────┐  │    INPUT STREAM (Commands)   │  ┌───────┐  │
│  │Writer │──┼─────────────────────────────▶│  │Reader │  │
│  └───────┘  │                              │  └───────┘  │
│             │                              │             │
│  ┌───────┐  │   OUTPUT STREAM (Responses)  │  ┌───────┐  │
│  │Reader │◀─┼──────────────────────────────┤  │Writer │  │
│  └───────┘  │                              │  └───────┘  │
│             │                              │             │
└─────────────┘                              └─────────────┘

Gateway writes NewOrder to Input Stream → Returns immediately
Gateway listens to Output Stream for responses (async)
Decouples request from response = natural backpressure handling
```

## Ring Buffer Bridge (LMAX Disruptor Style)

For maximum performance, use a ring buffer between Gateway and Risk:

```
                          Ring Buffer
                  ┌─────────────────────────┐
                  │ [0] [1] [2] [3] [4] ... │
                  └─────────────────────────┘
                       ▲               ▲
                       │               │
                  Write Cursor    Read Cursor
                       │               │
                  ┌────┴────┐     ┌────┴────┐
                  │ Gateway │     │  Risk   │
                  │(Producer│     │(Consumer│
                  └─────────┘     └─────────┘

Gateway: Claims slot, writes Order, advances Write Cursor
Risk Engine: Spins on Write Cursor, reads data, advances Read Cursor

In Rust: Use crossbeam-channel or raw Aeron streams
```

## Risk Engine Down: Fail Fast

Since routing is deterministic (User_A → Risk_4), if Risk_4 crashes:

```
1. DETECTION
   Gateway sees Risk_4 connection drop

2. FAIL FAST (Immediate!)
   Gateway rejects User_A's orders with 503 Service Unavailable
   DO NOT buffer - user/bot needs to know to retry

3. ORCHESTRATION
   Kubernetes/Nomad detects crash
   Starts new pod for Risk_4

4. HYDRATION
   New Risk_4 reads Snapshot + WAL from disk
   Rebuilds RAM state (takes 5-30 seconds)

5. RESUME
   Gateway reconnects to new Risk_4
   User_A can trade again
```

```rust
impl GatewayRouter {
    async fn send_to_risk(&self, user_id: u64, command: &[u8]) -> Result<(), Error> {
        let shard_index = (user_id % self.risk_nodes.len() as u64) as usize;
        let connection = &self.risk_nodes[shard_index];

        // Check if connection is healthy
        if !connection.is_connected() {
            // FAIL FAST - don't buffer, don't retry to different node
            return Err(Error::ServiceUnavailable(format!(
                "Risk shard {} unavailable, please retry",
                shard_index
            )));
        }

        connection.send(command).await
    }
}
```

## Implementation Checklist

| Step | Action | Notes |
|------|--------|-------|
| 1 | Parse Early | Convert JSON to binary in Gateway |
| 2 | Validate | Auth, signature, rate limits in Gateway |
| 3 | Route Deterministically | `user_id % shard_count` |
| 4 | Multiplex | Few persistent connections, many requests |
| 5 | No Logic | Gateway has ZERO business logic |
| 6 | Fail Fast | If shard down, 503 immediately |

## Gateway Design Principles

```rust
/// Gateway should be a "dumb pipe" with minimal responsibilities
struct Gateway {
    // Authentication
    api_key_validator: ApiKeyValidator,

    // Protocol normalization
    json_parser: JsonParser,
    binary_encoder: SbeEncoder,

    // Routing
    risk_connections: Vec<RiskConnection>,

    // Rate limiting
    rate_limiter: RateLimiter,

    // NO business logic here!
    // NO balance checking!
    // NO order validation beyond format!
}

impl Gateway {
    async fn handle_request(&self, raw_json: &str) -> Response {
        // 1. Parse (fast fail on bad JSON)
        let request = self.json_parser.parse(raw_json)?;

        // 2. Auth (fast fail on bad signature)
        self.api_key_validator.validate(&request)?;

        // 3. Rate limit (fast fail if exceeded)
        self.rate_limiter.check(request.user_id)?;

        // 4. Encode to binary (once, here)
        let binary = self.binary_encoder.encode(&request);

        // 5. Route and send (fire and forget)
        let shard = request.user_id % self.risk_connections.len() as u64;
        self.risk_connections[shard].send(&binary).await?;

        // 6. Response comes via separate output stream
        Ok(Response::Accepted)
    }
}
```

## Latency Budget

| Component | Target Latency | Notes |
|-----------|---------------|-------|
| TLS termination | 10-50 µs | Hardware acceleration helps |
| JSON parse | 1-5 µs | Use simd-json |
| Auth validation | 1-10 µs | Pre-computed HMAC |
| Binary encode | 0.1-1 µs | SBE is ~0 copy |
| Network to Risk | 10-50 µs | Aeron UDP |
| **Total** | **20-120 µs** | Gateway → Risk |

Compare to:
- Matching Engine: 1-10 µs
- Hot Path (ME → Risk): ~100 ns

Gateway ingress is typically the slowest part of the order path, but good design keeps it under 100 µs.
