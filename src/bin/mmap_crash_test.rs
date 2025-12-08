//! Isolated flush test - sleep between writes to force separate I/O ops

use std::fs::OpenOptions;
use memmap2::MmapMut;
use std::thread;
use std::time::Duration;

fn main() {
    let path = "/tmp/crash_test_isolated.wal";

    let _ = std::fs::remove_file(path);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .expect("Failed to open file");

    file.set_len(1024 * 1024).expect("set_len");

    let mut mmap = unsafe { MmapMut::map_mut(&file).expect("mmap") };

    let entry_size = 64;
    let num_entries = 10; // Fewer entries, but with delays
    let mut flush_times = Vec::new();

    for i in 0..num_entries {
        // Sleep to force separate I/O operations
        thread::sleep(Duration::from_millis(100));

        let offset = i * entry_size;
        let marker = format!("ENTRY_{:04}_ISOLATED_FLUSH_TEST_____", i);
        mmap[offset..offset + marker.len()].copy_from_slice(marker.as_bytes());

        let start = std::time::Instant::now();
        mmap.flush().expect("flush");
        let elapsed = start.elapsed().as_micros();
        flush_times.push(elapsed);
        println!("Entry {}: flush took {}µs", i, elapsed);
    }

    let avg: u128 = flush_times.iter().sum::<u128>() / flush_times.len() as u128;
    let min = flush_times.iter().min().unwrap();
    let max = flush_times.iter().max().unwrap();

    println!("\n=== Results ===");
    println!("Entries: {}", num_entries);
    println!("Min: {}µs, Avg: {}µs, Max: {}µs", min, avg, max);

    println!("\nNOT crashing - normal exit");
}
