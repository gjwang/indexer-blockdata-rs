# Aeron Communication Module

## Overview

UBSCore uses Aeron IPC for ultra-low latency (~100ns) communication between:
- Gateway → UBSCore (order requests)
- UBSCore → Matching Engine (validated orders)
- Matching Engine → UBSCore (trade fills)

## Installation

### 1. System Requirements (Build Time)

Since rusteron compiles the underlying C++ Aeron code, you need:

**macOS:**
```bash
brew install cmake llvm
```

**Ubuntu/Debian:**
```bash
sudo apt install cmake clang libclang-dev
```

### 2. Enable Aeron Feature

The Aeron module is gated behind a feature flag:

```bash
# Build with Aeron support
cargo build --features aeron

# Run tests with Aeron
cargo test --features aeron

# Build without Aeron (default)
cargo build
```

### 3. Verify Installation

```bash
# Check CMake version (needs 3.16+)
cmake --version

# Build example
cargo build --example aeron_hot_path --features aeron
```

## Usage

### Option A: Embedded Media Driver (Dev/Testing)

For development, the media driver runs inside your Rust process. This is simpler but the driver dies when your app crashes.

```rust
use rusteron_media_driver::{AeronDriver, AeronDriverContext};
use rusteron_client::{Aeron, AeronContext};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Configure and launch embedded Media Driver
    let media_context = AeronDriverContext::new()?;
    let (_stop_signal, _driver_handle) = AeronDriver::launch_embedded(
        media_context.clone(),
        false  // don't delete shm on exit
    );

    println!("Aeron Media Driver running at: /tmp/aeron-{}", whoami::username());

    // 2. Connect Aeron client
    let ctx = AeronContext::new()?;
    let aeron = Aeron::new(&ctx)?;
    aeron.start()?;

    println!("Aeron client connected!");

    // 3. Create publisher/subscriber...

    Ok(())
}
```

### Option B: Standalone Media Driver (Production)

For production, run the media driver as a separate process:

```bash
# Install the driver binary
cargo install rusteron-media-driver --features static

# Run the driver (keeps running)
media_driver

# Your app just connects without launch_embedded
```

## Wire Formats

### OrderMessage (40 bytes)
```rust
#[repr(C)]
pub struct OrderMessage {
    pub order_id: u64,      // Snowflake ID
    pub user_id: u64,       // User ID
    pub symbol_id: u32,     // Trading pair
    pub side: u8,           // 0 = Buy, 1 = Sell
    pub order_type: u8,     // 0 = Limit, 1 = Market
    pub price: u64,         // Price (raw u64)
    pub qty: u64,           // Quantity (raw u64)
}
```

### FillMessage (72 bytes)
```rust
#[repr(C)]
pub struct FillMessage {
    pub trade_id: u64,
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,
    pub is_maker: u8,
    pub price: u64,
    pub qty: u64,
    pub quote_qty: u64,
    pub fee: u64,
    pub timestamp_ns: u64,
}
```

## Stream Configuration

| Stream | ID | Direction |
|--------|-----|-----------|
| Orders In | 1001 | Gateway → UBSCore |
| Orders Out | 1002 | UBSCore → ME |
| Fills In | 1003 | ME → UBSCore |
| Responses Out | 1004 | UBSCore → Gateway |

Channel: `aeron:ipc` (shared memory via /tmp/aeron-*)

## Verifying Aeron is Running

```bash
# Linux
ls -l /dev/shm/ | grep aeron

# macOS
ls -l /tmp/ | grep aeron
```

## Troubleshooting

### CMake Version Error
```
CMake 3.30 or higher is required. You are running version 3.12.0
```
**Fix:** Update CMake: `brew upgrade cmake` or `apt upgrade cmake`

### uuid lib not found
**Fix (macOS):** `brew install ossp-uuid`
**Fix (Linux):** `apt install uuid-dev`

### Build takes too long
The first build compiles Aeron C++ code. Subsequent builds are cached.

## Performance Notes

- **IPC Latency:** ~100 nanoseconds
- **Network Latency:** ~10-50 microseconds (UDP multicast)
- **ZeroMQ Latency:** ~150-500 microseconds

Use `aeron:ipc` for same-machine communication (dev).
Use `aeron:udp?endpoint=...` for multi-machine (prod).

## Example: Run Hot Path Demo

```bash
# Build and run the example
cargo run --example aeron_hot_path --features aeron
```

This starts embedded media driver + publisher/subscriber demo.
