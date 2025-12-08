//! Performance benchmarks for UBSCore
//!
//! Run with: cargo test --lib ubs_core::bench --release

#[cfg(test)]
mod tests {
    use crate::fast_ulid::SnowflakeGenRng;
    use crate::ubs_core::{
        DeduplicationGuard, InternalOrder, LatencyTimer, OrderMetrics, OrderType, RejectReason,
        Side, SpotRiskModel, UBSCore,
    };

    const WARMUP_ITERATIONS: usize = 1000;
    const BENCH_ITERATIONS: usize = 100_000;

    fn make_order(ts: u64, seq: u16, user_id: u64) -> InternalOrder {
        InternalOrder {
            order_id: SnowflakeGenRng::from_parts(ts, 1, seq),
            user_id,
            symbol_id: 1,
            side: Side::Buy,
            price: 50000_00000000,
            qty: 1_00000000,
            order_type: OrderType::Limit,
        }
    }

    #[test]
    fn bench_dedup_check() {
        let mut dedup = DeduplicationGuard::new();
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;

        // Warmup
        for i in 0..WARMUP_ITERATIONS as u16 {
            let order_id = SnowflakeGenRng::from_parts(now, 1, i);
            let _ = dedup.check_and_record(order_id, now);
        }

        // Benchmark
        let timer = LatencyTimer::start();
        for i in WARMUP_ITERATIONS as u16..(WARMUP_ITERATIONS + BENCH_ITERATIONS) as u16 {
            let order_id = SnowflakeGenRng::from_parts(now, 1, i);
            let _ = dedup.check_and_record(order_id, now);
        }
        let elapsed = timer.elapsed_ns();

        let avg_ns = elapsed / BENCH_ITERATIONS as u64;
        println!("Dedup check: {} ns/op ({} ops)", avg_ns, BENCH_ITERATIONS);

        // Should be < 1µs
        assert!(avg_ns < 1000, "Dedup check too slow: {} ns", avg_ns);
    }

    #[test]
    fn bench_order_cost_calculation() {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;

        let order = make_order(now, 1, 1);

        // Warmup
        for _ in 0..WARMUP_ITERATIONS {
            let _ = order.calculate_cost();
        }

        // Benchmark
        let timer = LatencyTimer::start();
        for _ in 0..BENCH_ITERATIONS {
            let _ = order.calculate_cost();
        }
        let elapsed = timer.elapsed_ns();

        let avg_ns = elapsed / BENCH_ITERATIONS as u64;
        println!("Order cost calc: {} ns/op", avg_ns);

        // Should be < 100ns (just a multiply)
        assert!(avg_ns < 100, "Cost calc too slow: {} ns", avg_ns);
    }

    #[test]
    fn bench_metrics_recording() {
        let metrics = OrderMetrics::new();

        // Warmup
        for _ in 0..WARMUP_ITERATIONS {
            metrics.record_received();
            metrics.record_accepted(100);
        }
        metrics.reset();

        // Benchmark
        let timer = LatencyTimer::start();
        for i in 0..BENCH_ITERATIONS as u64 {
            metrics.record_received();
            metrics.record_accepted(i * 10);
        }
        let elapsed = timer.elapsed_ns();

        let avg_ns = elapsed / (BENCH_ITERATIONS as u64 * 2); // 2 ops per iteration
        println!("Metrics recording: {} ns/op", avg_ns);

        // Should be < 100ns (atomic ops)
        assert!(avg_ns < 500, "Metrics recording too slow: {} ns", avg_ns);
    }

    #[test]
    fn bench_ubscore_validate_order_rejection() {
        // Benchmark order validation (rejection path - no account)
        let mut core = UBSCore::new(SpotRiskModel);
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;

        // Benchmark - use different sequences and machine IDs to avoid dedup collision
        // seq is 13 bits (max 8191), machine_id is 7 bits (max 127)
        let timer = LatencyTimer::start();
        for i in 0..BENCH_ITERATIONS as u64 {
            // Use combination of machine_id and sequence to create unique order IDs
            let machine_id = ((i / 8192) % 128) as u8;
            let seq = (i % 8192) as u16;
            let order_id = SnowflakeGenRng::from_parts(now, machine_id, seq);
            let order = InternalOrder {
                order_id,
                user_id: i, // Different user each time
                symbol_id: 1,
                side: Side::Buy,
                price: 50000,
                qty: 100,
                order_type: OrderType::Limit,
            };
            let result = core.validate_order(&order);
            // Should fail on AccountNotFound (after dedup)
            assert!(result.is_err());
        }
        let elapsed = timer.elapsed_ns();

        let avg_ns = elapsed / BENCH_ITERATIONS as u64;
        let avg_us = avg_ns / 1000;
        println!("UBSCore validate_order (rejection): {} ns ({} µs) per order", avg_ns, avg_us);

        // Target: < 50µs
        assert!(avg_us < 50, "Order validation too slow: {} µs", avg_us);
    }

    #[test]
    fn bench_throughput() {
        let mut core = UBSCore::new(SpotRiskModel);
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64;

        let timer = LatencyTimer::start();
        let mut processed = 0u64;

        // Process as many orders as possible in 100ms
        loop {
            let order = make_order(now, (processed % 8192) as u16, processed);
            let _ = core.validate_order(&order);
            processed += 1;

            if timer.elapsed_us() > 100_000 {
                // 100ms
                break;
            }
        }

        let elapsed_secs = timer.elapsed_us() as f64 / 1_000_000.0;
        let throughput = processed as f64 / elapsed_secs;

        println!(
            "Throughput: {:.0} orders/sec ({} orders in {:.2}ms)",
            throughput,
            processed,
            elapsed_secs * 1000.0
        );

        // Target: > 100k orders/sec
        assert!(throughput > 100_000.0, "Throughput too low: {:.0}", throughput);
    }
}
