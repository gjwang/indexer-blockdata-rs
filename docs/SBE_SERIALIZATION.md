# SBE: Simple Binary Encoding for HFT

## What is SBE?

SBE (Simple Binary Encoding) is the **industry standard** serialization format for High-Frequency Trading. Developed by the FIX Trading Community (the standards body for world's stock markets).

**The key insight**: If JSON is "human-readable text" and Protobuf is "compact data," SBE is **"memory mapping."**

## The Core Concept: Zero-Copy Access

### JSON/Protobuf (Expensive)
```
Receive bytes → Allocate Memory → Loop through bytes →
Convert to Integers/Strings → Store in Object

Time: ~500 nanoseconds
Creates garbage for GC
```

### SBE (Instant)
```
Receive bytes → Cast a Pointer → Done

Time: ~15 nanoseconds (30x faster!)
Zero allocation
```

**The data on the wire is laid out exactly how the CPU wants it in RAM.**

## Visual Comparison

### JSON Payload
```json
{
  "id": 101,
  "price": 50000
}
```
CPU Work: Scan for `{`, scan for `"id"`, parse `:`, parse `101` (ASCII to Int), etc.

### SBE Payload
```
[Header: 4 bytes] [ID: 4 bytes] [Price: 8 bytes]
```
CPU Work:
```rust
let price = unsafe { *(buffer_ptr.add(8) as *const i64) };
```
Time: **1 CPU instruction.**

## SBE vs Protobuf

| Feature | Protobuf | SBE | Winner for HFT |
|---------|----------|-----|----------------|
| **Integer Encoding** | Varint (variable length) | Fixed length | SBE (Predictable) |
| **Parsing Cost** | CPU loops and branches | Direct memory read | SBE (Zero branching) |
| **Schema Changes** | Flexible (add anywhere) | Strict (append only) | Protobuf (Easier) |
| **Bandwidth** | Compact | Larger packets | Protobuf (Smaller) |
| **Latency** | ~500 ns | ~15 ns | **SBE (30x Faster)** |

**The Trade-off**: SBE uses more network bandwidth to save CPU time. In a datacenter, bandwidth is free, but CPU latency is everything.

## SBE Message Layout

```
┌────────────────────────────────────────────────────────────────────┐
│                         SBE Message                                  │
├──────────────────────┬──────────────────────────────────────────────┤
│  Message Header      │  Message Body (Fixed-Length Fields)          │
│  (8 bytes)           │                                              │
├──────────────────────┼────────┬────────┬────────┬────────┬─────────┤
│ BlockLen│TemplateId │ userId │symbolId│ price  │  qty   │  side   │
│ SchVer │ NumGroups  │ 8 bytes│ 8 bytes│ 8 bytes│ 8 bytes│ 1 byte  │
├──────────────────────┼────────┼────────┼────────┼────────┼─────────┤
│ Offset: 0            │    8   │   16   │   24   │   32   │   40    │
└──────────────────────┴────────┴────────┴────────┴────────┴─────────┘

Total: 41 bytes (fixed, always the same)
```

## Schema Definition (XML)

```xml
<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="exchange.messages"
                   id="1"
                   version="1"
                   byteOrder="littleEndian">

    <types>
        <type name="u64" primitiveType="uint64"/>
        <type name="u32" primitiveType="uint32"/>
        <type name="i64" primitiveType="int64"/>
        <type name="u8" primitiveType="uint8"/>
    </types>

    <!-- New Order Message -->
    <sbe:message name="NewOrder" id="100">
        <field name="userId"    id="1" type="u64"/>
        <field name="symbolId"  id="2" type="u32"/>
        <field name="orderId"   id="3" type="u64"/>
        <field name="price"     id="4" type="u64"/>
        <field name="quantity"  id="5" type="u64"/>
        <field name="side"      id="6" type="u8"/>   <!-- 0=Buy, 1=Sell -->
        <field name="orderType" id="7" type="u8"/>   <!-- 0=Limit, 1=Market -->
    </sbe:message>

    <!-- Trade Executed Message -->
    <sbe:message name="TradeExecuted" id="101">
        <field name="tradeId"      id="1" type="u64"/>
        <field name="symbolId"     id="2" type="u32"/>
        <field name="buyerUserId"  id="3" type="u64"/>
        <field name="sellerUserId" id="4" type="u64"/>
        <field name="buyerOrderId" id="5" type="u64"/>
        <field name="sellerOrderId"id="6" type="u64"/>
        <field name="price"        id="7" type="u64"/>
        <field name="quantity"     id="8" type="u64"/>
        <field name="timestampNs"  id="9" type="u64"/>
    </sbe:message>

    <!-- Balance Update (Hot Path) -->
    <sbe:message name="BalanceUpdate" id="102">
        <field name="eventId"   id="1" type="u64"/>
        <field name="userId"    id="2" type="u64"/>
        <field name="assetId"   id="3" type="u32"/>
        <field name="delta"     id="4" type="i64"/>  <!-- Positive=Credit, Negative=Debit -->
        <field name="timestamp" id="5" type="u64"/>
    </sbe:message>

</sbe:messageSchema>
```

## Rust Implementation: Zero-Copy Decoder

The SBE code generator creates "views" over byte buffers, not new structs:

```rust
/// SBE Decoder - Zero Copy!
/// This is a VIEW over the byte buffer, not a copy.
#[repr(C, packed)]
struct NewOrderDecoder<'a> {
    buffer: &'a [u8],
}

impl<'a> NewOrderDecoder<'a> {
    const USER_ID_OFFSET: usize = 8;  // After 8-byte header
    const SYMBOL_ID_OFFSET: usize = 16;
    const ORDER_ID_OFFSET: usize = 20;
    const PRICE_OFFSET: usize = 28;
    const QUANTITY_OFFSET: usize = 36;
    const SIDE_OFFSET: usize = 44;

    /// Create decoder over existing buffer (no allocation!)
    pub fn wrap(buffer: &'a [u8]) -> Self {
        Self { buffer }
    }

    /// Direct memory read - ONE CPU INSTRUCTION
    #[inline(always)]
    pub fn user_id(&self) -> u64 {
        let bytes: [u8; 8] = self.buffer[Self::USER_ID_OFFSET..Self::USER_ID_OFFSET+8]
            .try_into().unwrap();
        u64::from_le_bytes(bytes)
    }

    #[inline(always)]
    pub fn symbol_id(&self) -> u32 {
        let bytes: [u8; 4] = self.buffer[Self::SYMBOL_ID_OFFSET..Self::SYMBOL_ID_OFFSET+4]
            .try_into().unwrap();
        u32::from_le_bytes(bytes)
    }

    #[inline(always)]
    pub fn price(&self) -> u64 {
        let bytes: [u8; 8] = self.buffer[Self::PRICE_OFFSET..Self::PRICE_OFFSET+8]
            .try_into().unwrap();
        u64::from_le_bytes(bytes)
    }

    #[inline(always)]
    pub fn quantity(&self) -> u64 {
        let bytes: [u8; 8] = self.buffer[Self::QUANTITY_OFFSET..Self::QUANTITY_OFFSET+8]
            .try_into().unwrap();
        u64::from_le_bytes(bytes)
    }

    #[inline(always)]
    pub fn side(&self) -> u8 {
        self.buffer[Self::SIDE_OFFSET]
    }
}
```

## Rust Implementation: Zero-Copy Encoder

```rust
/// SBE Encoder - writes directly to buffer
struct NewOrderEncoder<'a> {
    buffer: &'a mut [u8],
}

impl<'a> NewOrderEncoder<'a> {
    const MESSAGE_SIZE: usize = 45;  // Header(8) + Body(37)
    const TEMPLATE_ID: u16 = 100;

    /// Wrap a mutable buffer for encoding
    pub fn wrap(buffer: &'a mut [u8]) -> Self {
        assert!(buffer.len() >= Self::MESSAGE_SIZE);
        Self { buffer }
    }

    /// Write header
    pub fn write_header(&mut self) {
        // Block length (u16)
        self.buffer[0..2].copy_from_slice(&37u16.to_le_bytes());
        // Template ID (u16)
        self.buffer[2..4].copy_from_slice(&Self::TEMPLATE_ID.to_le_bytes());
        // Schema version (u16)
        self.buffer[4..6].copy_from_slice(&1u16.to_le_bytes());
        // Num groups (u16)
        self.buffer[6..8].copy_from_slice(&0u16.to_le_bytes());
    }

    #[inline(always)]
    pub fn set_user_id(&mut self, value: u64) {
        self.buffer[8..16].copy_from_slice(&value.to_le_bytes());
    }

    #[inline(always)]
    pub fn set_symbol_id(&mut self, value: u32) {
        self.buffer[16..20].copy_from_slice(&value.to_le_bytes());
    }

    #[inline(always)]
    pub fn set_price(&mut self, value: u64) {
        self.buffer[28..36].copy_from_slice(&value.to_le_bytes());
    }

    #[inline(always)]
    pub fn set_quantity(&mut self, value: u64) {
        self.buffer[36..44].copy_from_slice(&value.to_le_bytes());
    }

    #[inline(always)]
    pub fn set_side(&mut self, value: u8) {
        self.buffer[44] = value;
    }

    /// Get the encoded bytes
    pub fn encoded_length(&self) -> usize {
        Self::MESSAGE_SIZE
    }
}
```

## Gateway Transmutation Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GATEWAY                                         │
│                                                                              │
│   1. INGRESS: Receive JSON from user bot                                    │
│      {"symbol": "BTC_USDT", "side": "BUY", "qty": "0.5", "price": "50000"}  │
│                                                                              │
│   2. VALIDATION: Check API key, signature                                   │
│                                                                              │
│   3. TRANSMUTATION (Critical Step!):                                        │
│      ┌─────────────────────────────────────────┐                            │
│      │ let mut buffer = [0u8; 45];             │                            │
│      │ let mut encoder = NewOrderEncoder::wrap(&mut buffer);                │
│      │ encoder.write_header();                 │                            │
│      │ encoder.set_user_id(1001);              │                            │
│      │ encoder.set_symbol_id(1);               │ // BTC_USDT                │
│      │ encoder.set_price(50000_00000000);      │ // Fixed point             │
│      │ encoder.set_quantity(50000000);         │ // 0.5 BTC in satoshi      │
│      │ encoder.set_side(0);                    │ // Buy                     │
│      └─────────────────────────────────────────┘                            │
│                                                                              │
│      NOTE: This is the LAST time data is transformed!                       │
│                                                                              │
│   4. TRANSPORT: Send raw SBE buffer via Aeron                               │
│      aeron_publication.offer(&buffer);                                       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Raw bytes (45 bytes, fixed)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            RISK ENGINE                                       │
│                                                                              │
│   No "parsing" - just wrap the buffer!                                       │
│                                                                              │
│   let decoder = NewOrderDecoder::wrap(&buffer);                              │
│   let user_id = decoder.user_id();   // 1 CPU instruction                   │
│   let price = decoder.price();        // 1 CPU instruction                   │
│                                                                              │
│   // Check balance, lock funds...                                            │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Where to Use SBE

| Path | Format | Reason |
|------|--------|--------|
| **Public API** | JSON/REST | Easy for users, bots, debugging |
| **VIP Market Makers** | SBE (optional) | They want lowest latency |
| **Gateway → Risk** | SBE | Internal, speed critical |
| **Risk → ME** | SBE | Internal, speed critical |
| **ME → Hot Path** | SBE | Speculative balance updates |
| **ME → Kafka (Cold)** | SBE or Bincode | Persistence, can be compact |

## Rust Crates for SBE

```toml
# Option 1: sbe-rs (pure Rust, with codegen)
# https://github.com/real-logic/simple-binary-encoding

# Option 2: Manual implementation (shown above)
# For maximum control and performance

# Option 3: bytemuck (for zero-copy struct casting)
[dependencies]
bytemuck = { version = "1", features = ["derive"] }
```

### Using bytemuck for Zero-Copy

```rust
use bytemuck::{Pod, Zeroable};

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C, packed)]
struct NewOrderMessage {
    // Header
    block_length: u16,
    template_id: u16,
    schema_version: u16,
    num_groups: u16,

    // Body
    user_id: u64,
    symbol_id: u32,
    order_id: u64,
    price: u64,
    quantity: u64,
    side: u8,
    order_type: u8,
    _padding: [u8; 2],  // Align to 8 bytes
}

impl NewOrderMessage {
    /// Zero-copy cast from bytes
    fn from_bytes(bytes: &[u8]) -> &Self {
        bytemuck::from_bytes(bytes)
    }

    /// Zero-copy cast to bytes
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

// Usage:
fn handle_message(buffer: &[u8]) {
    let msg = NewOrderMessage::from_bytes(buffer);  // Zero allocation!
    let user_id = msg.user_id;  // Direct memory read
    let price = msg.price;      // Direct memory read
}
```

## Summary

| Serialization | Parse Time | Use Case |
|---------------|------------|----------|
| JSON | 1-10 µs | Public API, debugging |
| Protobuf | 500 ns | General internal use |
| Bincode | 100-300 ns | Rust-to-Rust, persistence |
| **rkyv** | **~15 ns** | Rust-native zero-copy |
| **SBE** | **~15 ns** | HFT, cross-language |

**SBE is the only way to achieve consistent single-digit microsecond latency** because it removes "parsing" from the CPU entirely.

---

## Appendix: rkyv - The Rust-Native Alternative

**If you hate SBE's XML**, use `rkyv` (pronounced "archive").

rkyv is the "Rust-Native" answer to SBE:
- **Zero XML**: You define normal Rust structs
- **Zero Copy**: Same memory layout performance as SBE
- **Zero Parsing**: Cast a pointer, done

| Feature | SBE | rkyv | Bincode |
|---------|-----|------|---------|
| Schema Definition | Ugly XML | Rust Structs | Rust Structs |
| Read Speed | Instant (0ns) | Instant (0ns) | Fast (copy) |
| Developer Experience | Painful | Excellent | Excellent |
| Cross-Language | Yes (C++, Java) | No (Rust only) | No |

### rkyv Implementation

```toml
[dependencies]
rkyv = "0.7"
```

```rust
use rkyv::{Archive, Deserialize, Serialize};

// The macro calculates memory layout at compile time
#[derive(Archive, Deserialize, Serialize, Debug)]
#[archive(check_bytes)]
pub struct Order {
    pub user_id: u64,
    pub symbol_id: u32,
    pub price: u64,
    pub qty: u64,
    pub side: u8,
}

// Zero-Copy Read (The Magic)
fn on_fast_path_message(buffer: &[u8]) {
    // "Access" the buffer - effectively ZERO cost
    let archived = unsafe { rkyv::archived_root::<Order>(buffer) };

    // Read values - just memory offset lookup
    let uid = archived.user_id;
    let price = archived.price;

    println!("User {} bought at {}", uid, price);
}
```

### When to Use What

| Scenario | Choice |
|----------|--------|
| MVP/Prototype | Bincode (fastest dev time) |
| Pure Rust, max speed | rkyv |
| Cross-language (C++/Java interop) | SBE |
| Legacy system integration | SBE |

**Recommendation**:
1. Start with **Bincode** for prototype
2. If staying 100% Rust, switch to **rkyv**
3. If C++ interop needed, use **SBE**
