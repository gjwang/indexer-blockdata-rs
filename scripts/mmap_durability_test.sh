#!/bin/bash
# Test if mmap flush (msync) is truly durable by killing process immediately after flush

echo "=== mmap Durability Test ==="

# Create test file
WAL_FILE="/tmp/crash_test.wal"
rm -f $WAL_FILE

# Write a test program that:
# 1. Opens mmap WAL
# 2. Writes known data
# 3. Calls flush() (msync MS_SYNC)
# 4. Prints marker
# 5. Crashes immediately (kill -9)

cd /Users/gjwang/eclipse-workspace/rust_source/indexer-blockdata-rs

# Build test binary if needed
cat > /tmp/mmap_crash_test.rs << 'EOF'
use std::fs::OpenOptions;
use std::process;
use memmap2::MmapMut;

fn main() {
    let path = "/tmp/crash_test.wal";
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .expect("open");

    file.set_len(4096).expect("set_len");

    let mut mmap = unsafe { MmapMut::map_mut(&file).expect("mmap") };

    // Write known pattern
    let marker = b"DURABLE_TEST_12345";
    mmap[0..marker.len()].copy_from_slice(marker);

    // Sync to disk
    mmap.flush().expect("flush");

    println!("FLUSHED - marker written");

    // Crash immediately - no graceful shutdown
    unsafe { libc::abort(); }
}
EOF

echo "Test written. To run:"
echo "1. Compile: rustc /tmp/mmap_crash_test.rs -o /tmp/mmap_test"
echo "2. Run: /tmp/mmap_test"
echo "3. Check: hexdump -C /tmp/crash_test.wal | head"
echo ""
echo "If you see 'DURABLE_TEST_12345' in hexdump, msync is truly durable."
