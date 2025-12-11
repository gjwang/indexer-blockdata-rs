use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Simple Snowflake-style ID generator
/// Format: 44 bits timestamp (ms) + 20 bits sequence
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn generate_request_id() -> u64 {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64;

    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst) % (1 << 20);

    (timestamp_ms << 20) | seq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_request_id() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();

        assert_ne!(id1, id2);
        assert!(id1 > 0);
        assert!(id2 > 0);
    }

    #[test]
    fn test_unique_ids() {
        let ids: Vec<u64> = (0..1000).map(|_| generate_request_id()).collect();
        let unique_count = ids.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(unique_count, 1000);
    }
}
