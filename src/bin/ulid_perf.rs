use fetcher::fast_ulid::{FastUlidGen, FastUlidHalfGen};
use std::time::Instant;

fn main() {
    let iterations = 100_000_000; // Increased iterations for more stable results

    println!("Starting benchmark with {} iterations...", iterations);

    // 1. Benchmark FastUlidGen (u128 wrapper around ulid crate)
    let mut gen_128 = FastUlidGen::new();
    let start_128 = Instant::now();
    let mut _dummy_128 = 0;
    for _ in 0..iterations {
        // Prevent compiler optimization by using the result
        let val = gen_128.generate();
        _dummy_128 ^= u128::from(val);
    }
    let duration_128 = start_128.elapsed();

    // 2. Benchmark FastUlidHalfGen (u64 custom impl)
    let mut gen_64 = FastUlidHalfGen::new();
    let start_64 = Instant::now();
    let mut _dummy_64 = 0;
    for _ in 0..iterations {
        let val = gen_64.generate();
        _dummy_64 ^= val;
    }
    let duration_64 = start_64.elapsed();

    // Results
    println!("\n--- Results ---");

    println!("FastUlidGen (u128):");
    println!("  Total Time: {:?}", duration_128);
    println!(
        "  Throughput: {:.2} million ops/sec",
        iterations as f64 / duration_128.as_secs_f64() / 1_000_000.0
    );

    println!("FastUlidHalfGen (u64):");
    println!("  Total Time: {:?}", duration_64);
    println!(
        "  Throughput: {:.2} million ops/sec",
        iterations as f64 / duration_64.as_secs_f64() / 1_000_000.0
    );

    let speedup = duration_128.as_secs_f64() / duration_64.as_secs_f64();
    println!("\nComparison:");
    println!("  u64 is {:.2}x faster than u128", speedup);

    // Prevent optimization of the loops
    if _dummy_128 == 0 && _dummy_64 == 0 {
        println!("(Ignored)");
    }
}
