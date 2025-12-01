use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

pub struct FastUlidGen {
    generator: ulid::Generator,
}

impl Default for FastUlidGen {
    fn default() -> Self {
        Self::new()
    }
}

impl FastUlidGen {
    pub fn new() -> Self {
        Self {
            generator: ulid::Generator::new(),
        }
    }

    #[inline(always)]
    pub fn generate(&mut self) -> Ulid {
        // This uses Monotonic logic internally:
        // If time hasn't changed, it increments the random part.
        // It avoids the syscall overhead 99.9% of the time.
        self.generator.generate().unwrap()
    }
}

/// A 64-bit ID generator inspired by ULID.
/// Structure:
/// - 48 bits: Timestamp (milliseconds)
/// - 16 bits: Randomness / Counter
pub struct FastUlidHalfGen {
    last_val: u64,
    rng: rand::rngs::ThreadRng,
}

impl Default for FastUlidHalfGen {
    fn default() -> Self {
        Self::new()
    }
}

impl FastUlidHalfGen {
    pub fn new() -> Self {
        Self {
            last_val: 0,
            rng: rand::rng(),
        }
    }

    /// Generate a new unique u64 ID.
    /// Logic:
    /// 1. Get current timestamp (48 bits).
    /// 2. If timestamp > last_timestamp, use new timestamp and random 16 bits.
    /// 3. If timestamp == last_timestamp (or clock moved back), increment last value.
    pub fn generate(&mut self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_millis() as u64;

        // 48 bits timestamp, shifted to high bits
        let ts_part = now << 16;
        // Get the timestamp part of the last value
        let last_ts_part = self.last_val & 0xFFFF_FFFF_FFFF_0000;

        if ts_part > last_ts_part {
            // New millisecond: generate random low 16 bits
            let rand_part = self.rng.random::<u16>() as u64;
            self.last_val = ts_part | rand_part;
        } else {
            // Same millisecond or regression: increment
            // This might overflow into the timestamp bits if we exhaust 16 bits (65536 IDs/ms),
            // which effectively moves us to the "next" millisecond in ID space.
            self.last_val = self.last_val.wrapping_add(1);
        }
        self.last_val
    }

    /// 7.1 Serialize as ULID-like Base32 (Crockford)
    /// Format: TTTTTTTTTTRRRR (10 chars timestamp, 4 chars random)
    /// This ensures it looks like a standard ULID (starts with 01...) and sorts lexicographically.
    pub fn to_str_base32(val: u64) -> String {
        const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        
        let ts = val >> 16;
        let rand = val & 0xFFFF;

        let mut chars = vec!['0'; 14];

        // Encode Timestamp (10 chars)
        let mut t = ts;
        for i in (0..10).rev() {
            chars[i] = ALPHABET[(t % 32) as usize] as char;
            t /= 32;
        }

        // Encode Randomness (4 chars)
        let mut r = rand;
        for i in (10..14).rev() {
            chars[i] = ALPHABET[(r % 32) as usize] as char;
            r /= 32;
        }

        chars.into_iter().collect()
    }

    /// 7.2 Serialize as Decimal String
    pub fn to_str_decimal(val: u64) -> String {
        val.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_fast_ulid_half_gen_monotonicity() {
        let mut gen = FastUlidHalfGen::new();
        let mut last = gen.generate();
        
        for _ in 0..10000 {
            let next = gen.generate();
            assert!(next > last, "IDs must be strictly increasing. Last: {}, Next: {}", last, next);
            last = next;
        }
    }

    #[test]
    fn test_fast_ulid_half_gen_uniqueness() {
        let mut gen = FastUlidHalfGen::new();
        let mut set = HashSet::new();
        for _ in 0..1_000_000 {
            let id = gen.generate();
            assert!(set.insert(id), "Duplicate ID generated: {}", id);
        }
    }

    #[test]
    fn test_serialization() {
        let val = 0x0123456789ABCDEF; // Example value
        let b32 = FastUlidHalfGen::to_str_base32(val);
        let dec = FastUlidHalfGen::to_str_decimal(val);
        
        println!("Val: {}, Base32: {}, Dec: {}", val, b32, dec);
        assert!(!b32.is_empty());
        assert!(!dec.is_empty());
        
        // Verify Base32 chars
        assert!(b32.chars().all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)));
    }
    #[test]
    fn demo_usage_half_ulid_gen() {
        let mut gen = FastUlidHalfGen::new();
        println!("\n--- FastUlidHalfGen Demo ---");
        println!("{:<20} | {:<15} | {:<20}", "u64 (Decimal)", "Base32", "Timestamp (ms)");
        println!("{:-<20}-+-{:-<15}-+-{:-<20}", "", "", "");

        for _ in 0..5 {
            let id = gen.generate();
            let b32 = FastUlidHalfGen::to_str_base32(id);
            let dec = FastUlidHalfGen::to_str_decimal(id);
            let ts = id >> 16;
            
            println!("{:<20} | {:<15} | {:<20}", dec, b32, ts);
        }
        println!("----------------------------\n");
    }

    #[test]
    fn demo_usage_ulid_gen() {
        let mut gen = FastUlidGen::new();
        println!("\n--- FastUlidGen Demo ---");
        println!("{:<20} | {:<15} | {:<20}", "u128 (Decimal)", "Base32", "Timestamp (ms)");
        println!("{:-<20}-+-{:-<15}-+-{:-<20}", "", "", "");

        for _ in 0..5 {
            let ulid = gen.generate();
            let dec = ulid.to_string();
            let ts = ulid.timestamp_ms();
            
            println!("{:<20} | {:<15} | {:<20}", dec, ulid, ts);
        }
        println!("----------------------------\n");
    }

}
