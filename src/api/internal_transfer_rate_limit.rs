// Rate limiting for internal transfers
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Rate limiter using token bucket algorithm
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<u64, TokenBucket>>>,
    max_tokens: u32,
    refill_rate: u32, // tokens per second
    window: Duration,
}

struct TokenBucket {
    tokens: u32,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(max_tokens: u32, refill_rate: u32, window_secs: u64) -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
            max_tokens,
            refill_rate,
            window: Duration::from_secs(window_secs),
        }
    }

    /// Check if request is allowed for user
    pub fn check_rate_limit(&self, user_id: u64) -> Result<(), RateLimitError> {
        let mut buckets = self.buckets.lock().unwrap();

        let bucket = buckets.entry(user_id).or_insert_with(|| TokenBucket {
            tokens: self.max_tokens,
            last_refill: Instant::now(),
        });

        // Refill tokens based on elapsed time
        let elapsed = bucket.last_refill.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let refill_amount = (elapsed.as_secs() as u32) * self.refill_rate;
            bucket.tokens = (bucket.tokens + refill_amount).min(self.max_tokens);
            bucket.last_refill = Instant::now();
        }

        // Check if tokens available
        if bucket.tokens > 0 {
            bucket.tokens -= 1;
            Ok(())
        } else {
            Err(RateLimitError {
                user_id,
                retry_after_secs: 1,
            })
        }
    }

    /// Get remaining quota for user
    pub fn get_remaining_quota(&self, user_id: u64) -> u32 {
        let buckets = self.buckets.lock().unwrap();
        buckets.get(&user_id).map(|b| b.tokens).unwrap_or(self.max_tokens)
    }

    /// Reset rate limit for user (admin function)
    pub fn reset_user(&self, user_id: u64) {
        let mut buckets = self.buckets.lock().unwrap();
        buckets.remove(&user_id);
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitError {
    pub user_id: u64,
    pub retry_after_secs: u64,
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rate limit exceeded for user {}. Retry after {} seconds",
            self.user_id, self.retry_after_secs
        )
    }
}

impl std::error::Error for RateLimitError {}

/// Transfer limits configuration
pub struct TransferLimits {
    pub min_amount: i64,
    pub max_amount: i64,
    pub daily_limit: i64,
    pub per_transfer_limit: i64,
}

impl TransferLimits {
    pub fn default() -> Self {
        Self {
            min_amount: 1_000_000,        // 0.01 USDT (8 decimals)
            max_amount: 100_000_000_000_000, // 1M USDT
            daily_limit: 1_000_000_000_000_000, // 10M USDT per day
            per_transfer_limit: 100_000_000_000_000, // 1M USDT per transfer
        }
    }

    pub fn check_amount(&self, amount: i64) -> Result<(), String> {
        if amount < self.min_amount {
            return Err(format!(
                "Amount {} below minimum {}",
                amount, self.min_amount
            ));
        }

        if amount > self.max_amount {
            return Err(format!(
                "Amount {} exceeds maximum {}",
                amount, self.max_amount
            ));
        }

        Ok(())
    }
}

/// Daily transfer tracker
pub struct DailyTransferTracker {
    transfers: Arc<Mutex<HashMap<u64, DailyStats>>>,
}

struct DailyStats {
    total_amount: i64,
    transfer_count: u32,
    last_reset: Instant,
}

impl DailyTransferTracker {
    pub fn new() -> Self {
        Self {
            transfers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn record_transfer(&self, user_id: u64, amount: i64) -> Result<(), String> {
        let mut transfers = self.transfers.lock().unwrap();

        let stats = transfers.entry(user_id).or_insert_with(|| DailyStats {
            total_amount: 0,
            transfer_count: 0,
            last_reset: Instant::now(),
        });

        // Reset if 24 hours passed
        if stats.last_reset.elapsed() >= Duration::from_secs(86400) {
            stats.total_amount = 0;
            stats.transfer_count = 0;
            stats.last_reset = Instant::now();
        }

        stats.total_amount += amount;
        stats.transfer_count += 1;

        Ok(())
    }

    pub fn get_daily_stats(&self, user_id: u64) -> (i64, u32) {
        let transfers = self.transfers.lock().unwrap();
        transfers
            .get(&user_id)
            .map(|s| (s.total_amount, s.transfer_count))
            .unwrap_or((0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(5, 1, 60);

        // Should allow first 5 requests
        for _ in 0..5 {
            assert!(limiter.check_rate_limit(100).is_ok());
        }

        // 6th request should fail
        assert!(limiter.check_rate_limit(100).is_err());
    }

    #[test]
    fn test_token_refill() {
        let limiter = RateLimiter::new(2, 2, 60);

        // Use all tokens
        limiter.check_rate_limit(100).unwrap();
        limiter.check_rate_limit(100).unwrap();
        assert!(limiter.check_rate_limit(100).is_err());

        // Wait for refill
        thread::sleep(Duration::from_millis(1100));

        // Should have tokens again
        assert!(limiter.check_rate_limit(100).is_ok());
    }

    #[test]
    fn test_transfer_limits() {
        let limits = TransferLimits::default();

        assert!(limits.check_amount(10_000_000).is_ok()); // Valid
        assert!(limits.check_amount(100).is_err()); // Too small
        assert!(limits.check_amount(999_999_999_999_999).is_err()); // Too large
    }

    #[test]
    fn test_daily_tracker() {
        let tracker = DailyTransferTracker::new();

        tracker.record_transfer(100, 1_000_000_000).unwrap();
        tracker.record_transfer(100, 500_000_000).unwrap();

        let (total, count) = tracker.get_daily_stats(100);
        assert_eq!(total, 1_500_000_000);
        assert_eq!(count, 2);
    }
}
