# WAL Safety: The Critical Safety Mechanism

> **If UBSCore crashes and the WAL is corrupt or missing data, you have lost user money.**

This document covers how to build a production-grade, crash-safe Write-Ahead Log.

## The Latency Hierarchy

| Method | Latency | Safety | What Happens |
|--------|---------|--------|--------------|
| Standard `write()` | ~200 ns | ❌ Unsafe | Copy to kernel RAM (Page Cache) |
| O_DIRECT + fsync (Consumer NVMe) | 40-200 µs | ✅ Safe | Commit to NAND Flash |
| O_DIRECT + fsync (Enterprise NVMe) | 10-20 µs | ✅ Safe | Commit to Drive's PLP RAM |
| Intel Optane (P5800X) | 3-5 µs | ✅✅ Safe | Commit to 3D XPoint Media |

## 1. Physical Write Strategy: O_DIRECT + fsync

### Why Not Standard write()?

```
Standard write() Path:
App → Kernel Page Cache (RAM) → [OS decides when] → NVMe

Problem: Power cut before OS flushes = DATA LOST
```

### O_DIRECT: Bypass the OS Cache

```rust
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

// O_DIRECT: Bypasses OS Page Cache
// Data goes: App RAM → NVMe controller directly
let file = OpenOptions::new()
    .write(true)
    .append(true)
    .custom_flags(libc::O_DIRECT)
    .open("ubs_wal.log")?;

fn append_event(file: &mut File, data: &[u8]) -> std::io::Result<()> {
    file.write_all(data)?;

    // FSYNC: Forces NVMe controller to flush to NAND
    // Blocks until bits are magnetized
    file.sync_data()?;

    Ok(())
}
```

### The Alignment Requirement (Critical!)

O_DIRECT **crashes** if your buffer isn't aligned:
- Buffer address: Must be multiple of block size (4096)
- Write size: Must be multiple of block size (4096)

```rust
use std::alloc::{alloc, dealloc, Layout};
use std::slice;

/// Page-aligned buffer for O_DIRECT
pub struct AlignedBuffer {
    ptr: *mut u8,
    layout: Layout,
    capacity: usize,
    len: usize,
}

impl AlignedBuffer {
    const ALIGNMENT: usize = 4096;

    pub fn new(size: usize) -> Self {
        // Round up to alignment
        let capacity = (size + Self::ALIGNMENT - 1) & !(Self::ALIGNMENT - 1);
        let layout = Layout::from_size_align(capacity, Self::ALIGNMENT).unwrap();
        let ptr = unsafe { alloc(layout) };

        // Zero-initialize
        unsafe { std::ptr::write_bytes(ptr, 0, capacity) };

        Self { ptr, layout, capacity, len: 0 }
    }

    pub fn write(&mut self, data: &[u8]) -> bool {
        if self.len + data.len() > self.capacity {
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.ptr.add(self.len),
                data.len()
            );
        }
        self.len += data.len();
        true
    }

    pub fn as_slice(&self) -> &[u8] {
        // Return full 4KB block (padded with zeros)
        unsafe { slice::from_raw_parts(self.ptr, self.capacity) }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        unsafe { std::ptr::write_bytes(self.ptr, 0, self.capacity) };
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) };
    }
}
```

## 2. O_DIRECT vs Standard write() vs mmap

### Standard write() + fsync (Dangerous)

```
Path: App → memcpy → Kernel Page Cache → [shared with other processes] → NVMe

The Hidden Killer: "Dirty Page Spike"
- Your UBSCore is running smoothly
- Metrics Agent writes 500MB of logs
- OS Page Cache fills up
- OS decision: "Flush ALL dirty pages NOW"
- Your fsync() blocks behind 500MB of logs
- Result: 10µs latency spikes to 50ms
```

### O_DIRECT + fsync (Recommended)

```
Path: App → DMA → NVMe (private lane, no traffic)

Benefits:
- Consistent latency (no jitter from OS background flush)
- Lower CPU usage (DMA, no memcpy)
- Full control over write timing
```

### mmap + msync (Trap!)

**Do NOT use mmap for the WAL write path.**

| Problem | Description |
|---------|-------------|
| Page Faults | Appending triggers Major Page Fault (5-50µs spike) |
| msync overhead | Must scan Page Table for dirty pages |
| TLB Shootdowns | Multi-core: Must interrupt ALL CPUs |
| Corruption Risk | Stray pointer can overwrite WAL data |

**When mmap IS good**: Reading/replaying the log (not writing).

## 3. Data Integrity: CRC32 Checksums

### The Problem: Torn Writes

Power cuts mid-write → half an order on disk → indistinguishable from garbage.

### The Solution: Framed Entries

```
┌─────────────────────────────────────────────────────────┐
│                    WAL Entry Format                      │
├──────────────┬──────────────┬───────────────────────────┤
│ Length (4B)  │ CRC32 (4B)   │ Payload (N bytes)         │
├──────────────┼──────────────┼───────────────────────────┤
│ 0x00000064   │ 0xABCD1234   │ [Order Data: 100 bytes]   │
└──────────────┴──────────────┴───────────────────────────┘
```

### Recovery Logic (On Startup)

```rust
use crc32fast::Hasher;

pub struct WalEntry {
    pub length: u32,
    pub crc32: u32,
    pub payload: Vec<u8>,
}

impl WalEntry {
    pub fn encode(payload: &[u8]) -> Vec<u8> {
        let length = payload.len() as u32;

        // Calculate CRC32
        let mut hasher = Hasher::new();
        hasher.update(payload);
        let crc32 = hasher.finalize();

        // Build frame
        let mut frame = Vec::with_capacity(8 + payload.len());
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(&crc32.to_le_bytes());
        frame.extend_from_slice(payload);

        frame
    }

    pub fn decode(buffer: &[u8]) -> Result<(WalEntry, usize), WalError> {
        if buffer.len() < 8 {
            return Err(WalError::Incomplete);
        }

        let length = u32::from_le_bytes(buffer[0..4].try_into().unwrap()) as usize;
        let stored_crc = u32::from_le_bytes(buffer[4..8].try_into().unwrap());

        if buffer.len() < 8 + length {
            return Err(WalError::Incomplete);
        }

        let payload = &buffer[8..8 + length];

        // Verify CRC32
        let mut hasher = Hasher::new();
        hasher.update(payload);
        let computed_crc = hasher.finalize();

        if stored_crc != computed_crc {
            return Err(WalError::Corrupted);
        }

        Ok((
            WalEntry {
                length: length as u32,
                crc32: stored_crc,
                payload: payload.to_vec(),
            },
            8 + length, // bytes consumed
        ))
    }
}

#[derive(Debug)]
pub enum WalError {
    Incomplete,  // Need more bytes
    Corrupted,   // CRC mismatch - crash point detected
}

/// Recovery: Read until first CRC error, then stop
pub fn recover_wal(path: &str) -> Vec<WalEntry> {
    let data = std::fs::read(path).unwrap_or_default();
    let mut entries = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        match WalEntry::decode(&data[offset..]) {
            Ok((entry, consumed)) => {
                entries.push(entry);
                offset += consumed;
            }
            Err(WalError::Incomplete) => {
                // Partial write at end - truncate file here
                eprintln!("WAL: Truncating incomplete entry at offset {}", offset);
                break;
            }
            Err(WalError::Corrupted) => {
                // CRC mismatch - this is the crash point
                eprintln!("WAL: Corruption detected at offset {}", offset);
                break;
            }
        }
    }

    entries
}
```

## 4. Group Commit (Batching)

**The Problem**: fsync is slow (10-200µs). If you fsync every order, max throughput = 5,000-100,000 orders/sec.

**The Solution**: Batch multiple orders, fsync once.

```rust
use std::time::{Duration, Instant};

pub struct GroupCommitWal {
    file: File,
    buffer: AlignedBuffer,
    pending_count: usize,
    last_sync: Instant,
}

impl GroupCommitWal {
    const MAX_BATCH_SIZE: usize = 50;
    const MAX_BATCH_DELAY: Duration = Duration::from_micros(100);

    pub fn new(path: &str) -> Self {
        let file = OpenOptions::new()
            .write(true)
            .append(true)
            .create(true)
            .custom_flags(libc::O_DIRECT)
            .open(path)
            .unwrap();

        Self {
            file,
            buffer: AlignedBuffer::new(64 * 1024), // 64KB buffer
            pending_count: 0,
            last_sync: Instant::now(),
        }
    }

    /// Append an entry (may not be persisted yet)
    pub fn append(&mut self, payload: &[u8]) {
        let frame = WalEntry::encode(payload);
        self.buffer.write(&frame);
        self.pending_count += 1;
    }

    /// Check if batch should be committed
    pub fn should_sync(&self) -> bool {
        self.pending_count >= Self::MAX_BATCH_SIZE
            || self.last_sync.elapsed() > Self::MAX_BATCH_DELAY
    }

    /// Commit the batch to disk
    pub fn sync(&mut self) -> std::io::Result<()> {
        if self.pending_count == 0 {
            return Ok(());
        }

        // Write the full aligned buffer
        self.file.write_all(self.buffer.as_slice())?;

        // FSYNC: The money shot
        self.file.sync_data()?;

        // Reset
        self.buffer.clear();
        self.pending_count = 0;
        self.last_sync = Instant::now();

        Ok(())
    }
}

// Usage in UBSCore main loop:
fn process_batch(wal: &mut GroupCommitWal, orders: Vec<Order>) {
    // 1. Process all orders in RAM
    for order in &orders {
        // Update in-memory state...

        // Append to WAL buffer (not synced yet)
        wal.append(&order.to_bytes());
    }

    // 2. Single fsync for the whole batch
    wal.sync().unwrap();

    // 3. ACK all users
    for order in &orders {
        send_ack(order.client_id);
    }
}

// Effective latency per order:
// Total: 12 µs (RAM + sync)
// Orders: 50
// Per Order: 0.24 µs
```

## 5. Dual Disk (Software RAID-1)

**Hardware fails.** Don't rely on OS RAID. Mirror in application code.

```rust
use std::fs::File;
use std::io::Write;

pub struct DualWal {
    primary: File,    // /mnt/nvme0/ubs.wal
    secondary: File,  // /mnt/nvme1/ubs.wal
}

impl DualWal {
    pub fn new(primary_path: &str, secondary_path: &str) -> Self {
        let flags = libc::O_DIRECT;

        let primary = OpenOptions::new()
            .write(true).append(true).create(true)
            .custom_flags(flags)
            .open(primary_path).unwrap();

        let secondary = OpenOptions::new()
            .write(true).append(true).create(true)
            .custom_flags(flags)
            .open(secondary_path).unwrap();

        Self { primary, secondary }
    }

    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        // Write to both
        self.primary.write_all(data)?;
        self.secondary.write_all(data)?;

        // Sync both (parallel would be better with threads)
        self.primary.sync_data()?;
        self.secondary.sync_data()?;

        Ok(())
    }
}

// If nvme0 dies, restart pointing to nvme1
```

## 6. Disaster Recovery: Async Replication

**What if the entire server catches fire?**

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              UBSCore                                         │
│                                                                              │
│   1. Write to Local WAL (NVMe)     ─────────────────────────────┐           │
│                                                                  │           │
│   2. ACK to User ◄──────────────────────────────────────────────┼───────────│
│                                                                  │           │
│   3. Background "Shipper" Thread   ◄────────────────────────────┘           │
│      - Tails WAL file                                                        │
│      - Pushes to Redpanda / Standby                                         │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ Async (Pipelined)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Redpanda / Standby Node                              │
│                                                                              │
│   Receives WAL entries with ~5ms delay                                       │
│   Can replay to rebuild UBSCore state                                        │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘

Risk: In datacenter fire, lose last few milliseconds ("In-Flight Gap")
Mitigation: Acceptable for Spot (rollback to checkpoint)
Alternative: Synchronous Replication (wait for Standby ACK, doubles latency)
```

## 7. The "Tail -f" Pattern: Log Shipper

The Shipper needs to continuously read new data from the WAL:

```rust
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::thread;
use std::time::Duration;

pub struct WalShipper {
    path: String,
    file: File,
    offset: u64,
    inode: u64,
}

impl WalShipper {
    pub fn new(path: &str) -> Self {
        let file = File::open(path).expect("Could not open WAL");
        let inode = file.metadata().unwrap().ino();

        Self {
            path: path.to_string(),
            file,
            offset: 0,
            inode,
        }
    }

    pub fn run_loop(&mut self) {
        let mut buffer = [0u8; 40960]; // 40KB chunks

        loop {
            // 1. Seek to our last position
            self.file.seek(SeekFrom::Start(self.offset)).unwrap();

            // 2. Try to read
            match self.file.read(&mut buffer) {
                Ok(0) => {
                    // EOF - no new data yet

                    // Check for log rotation
                    if self.check_rotation() {
                        continue; // Reopened file, try reading again
                    }

                    // No new data, sleep
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(n) => {
                    // New data!
                    let new_data = &buffer[..n];

                    // Process in frames (handle torn reads)
                    let valid_bytes = self.process_and_ship(new_data);

                    // Only advance offset by fully processed bytes
                    self.offset += valid_bytes as u64;
                }
                Err(e) => {
                    eprintln!("Error reading WAL: {}", e);
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    /// Check if log file was rotated (new inode)
    fn check_rotation(&mut self) -> bool {
        let metadata = std::fs::metadata(&self.path).ok();

        if let Some(meta) = metadata {
            if meta.ino() != self.inode {
                // File rotated! Reopen
                eprintln!("WAL rotated, reopening");

                // Finish reading current file first
                self.drain_current_file();

                // Open new file
                self.file = File::open(&self.path).unwrap();
                self.inode = meta.ino();
                self.offset = 0;

                return true;
            }
        }

        false
    }

    fn drain_current_file(&mut self) {
        // Read any remaining data from the old file
        let mut buffer = Vec::new();
        self.file.read_to_end(&mut buffer).ok();
        if !buffer.is_empty() {
            self.process_and_ship(&buffer);
        }
    }

    /// Process frames and ship to Redpanda/S3
    /// Returns number of bytes successfully processed
    fn process_and_ship(&self, data: &[u8]) -> usize {
        let mut offset = 0;
        let mut batch = Vec::new();

        while offset < data.len() {
            // Check if we have a complete frame
            if data.len() - offset < 8 {
                break; // Not enough for header
            }

            let length = u32::from_le_bytes(
                data[offset..offset+4].try_into().unwrap()
            ) as usize;

            if data.len() - offset < 8 + length {
                break; // Incomplete payload - stop here
            }

            // Full frame available
            let frame = &data[offset..offset + 8 + length];
            batch.push(frame.to_vec());
            offset += 8 + length;
        }

        // Ship the batch
        if !batch.is_empty() {
            self.ship_to_redpanda(&batch);
        }

        offset // Return how many bytes we consumed
    }

    fn ship_to_redpanda(&self, frames: &[Vec<u8>]) {
        // TODO: Produce to Kafka/Redpanda
        println!("Shipping {} frames to Redpanda", frames.len());
    }
}
```

## 8. Hardware Recommendations

| Component | Requirement | Why |
|-----------|-------------|-----|
| **NVMe** | Enterprise with PLP | Power Loss Protection capacitors |
| **Examples** | Samsung PM1735, Micron 9400, Intel Optane P5800X | PLP RAM allows instant "ack" |
| **Consumer SSDs** | ❌ NEVER | Data lost on power cut even after fsync |

### Advanced: SPDK (Storage Performance Development Kit)

For sub-10µs latency, bypass the Linux kernel entirely:

```
Standard O_DIRECT:
App → System Call → Kernel → NVMe Driver → Device
Overhead: ~3-5 µs (context switches, interrupts)

SPDK:
App → User-Space Driver → Device
Overhead: ~0.5 µs (polling, zero context switches)
```

## Summary Checklist

| Item | Recommendation |
|------|----------------|
| **Format** | Length + CRC32 + Payload |
| **IO** | O_DIRECT + fdatasync |
| **Batching** | Group Commit (10-50 events) |
| **Recovery** | Read until first CRC error, truncate |
| **Hardware** | Enterprise NVMe with PLP |
| **Replication** | Async to Redpanda/Standby |
| **Dual Disk** | Application-level RAID-1 |

## Comparison Summary

| Feature | Standard write | O_DIRECT | mmap |
|---------|---------------|----------|------|
| Throughput | High | High (batched) | High |
| Latency (avg) | Low | Low | Low |
| **Latency (p99)** | **Terrible (spikes)** | **Excellent** | Unpredictable |
| CPU Usage | High (memcpy) | Low (DMA) | Medium |
| Control | Low | High | Low |
| Complexity | Easy | Hard (alignment) | Easy |

**Verdict**: Use O_DIRECT for the WAL write path. The consistency is worth the complexity.
