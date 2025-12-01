use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;
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

        if ts_part > self.last_val {
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

    /// Extract the timestamp part (milliseconds since epoch)
    pub fn timestamp_ms(val: u64) -> u64 {
        val >> 16
    }

    /// Extract the random/counter part
    pub fn random_part(val: u64) -> u16 {
        (val & 0xFFFF) as u16
    }

    /// Create a u64 ID from timestamp and random part
    pub fn from_parts(timestamp_ms: u64, random_part: u16) -> u64 {
        (timestamp_ms << 16) | (random_part as u64)
    }

    /// 7.2 Serialize as Decimal String
    pub fn to_str_decimal(val: u64) -> String {
        val.to_string()
    }
}

/// A 64-bit Snowflake ID generator.
/// Structure:
/// - 44 bits: Timestamp (milliseconds)
///   - Max time: 2^44 ms = ~557 years from epoch (1970)
///   - Valid until: Year 2527
/// - 8 bits: Machine ID
///   - Capacity: 2^8 = 256 unique machines/gateways
/// - 12 bits: Sequence
///   - Capacity: 2^12 = 4096 IDs per millisecond per machine
///   - Throughput: ~4 million IDs/sec per machine
pub struct SnowflakeGen {
    machine_id: u8,
    last_ts: u64,
    sequence: u16,
}

impl SnowflakeGen {
    pub fn new(machine_id: u8) -> Self {
        Self {
            machine_id,
            last_ts: 0,
            sequence: 0,
        }
    }

    pub fn generate(&mut self) -> u64 {
        let mut now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_millis() as u64;

        if now < self.last_ts {
            // Clock moved backwards: use last_ts to maintain monotonicity
            now = self.last_ts;
        }

        if now == self.last_ts {
            self.sequence = (self.sequence + 1) & 0xFFF; // 12 bits mask (4095)
            if self.sequence == 0 {
                // Overflow (sequence > 4095), wait for next millisecond
                while now <= self.last_ts {
                    now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or(std::time::Duration::ZERO)
                        .as_millis() as u64;
                }
            }
        } else {
            self.sequence = 0;
        }

        self.last_ts = now;

        // Construct ID: TS (44) | Machine (8) | Seq (12)
        (now << 20) | ((self.machine_id as u64) << 12) | (self.sequence as u64)
    }

    /// Extract the timestamp part (milliseconds since epoch)
    pub fn timestamp_ms(val: u64) -> u64 {
        val >> 20
    }

    /// Extract the machine ID part
    pub fn machine_id(val: u64) -> u8 {
        ((val >> 12) & 0xFF) as u8
    }

    /// Extract the sequence part
    pub fn sequence(val: u64) -> u16 {
        (val & 0xFFF) as u16
    }

    /// Create a u64 Snowflake ID from parts
    pub fn from_parts(timestamp_ms: u64, machine_id: u8, sequence: u16) -> u64 {
        (timestamp_ms << 20) | ((machine_id as u64) << 12) | ((sequence & 0xFFF) as u64)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn test_fast_ulid_half_gen_monotonicity() {
        let mut gen = FastUlidHalfGen::new();
        let mut last = gen.generate();

        for _ in 0..10000 {
            let next = gen.generate();
            assert!(
                next > last,
                "IDs must be strictly increasing. Last: {}, Next: {}",
                last,
                next
            );
            last = next;
        }
    }

    #[test]
    fn test_fast_ulid_half_gen_uniqueness() {
        let mut gen = FastUlidHalfGen::new();
        let mut set = HashSet::new();
        for _ in 0..10000 {
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
        assert!(b32
            .chars()
            .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)));
    }
    #[test]
    fn demo_usage_half_ulid_gen() {
        let mut gen = FastUlidHalfGen::new();
        println!("\n--- FastUlidHalfGen Demo ---");
        println!(
            "{:<20} | {:<15} | {:<20}",
            "u64 (Decimal)", "Base32", "Timestamp (ms)"
        );
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
        println!(
            "{:<20} | {:<15} | {:<20}",
            "u128 (Decimal)", "Base32", "Timestamp (ms)"
        );
        println!("{:-<20}-+-{:-<15}-+-{:-<20}", "", "", "");

        for _ in 0..5 {
            let ulid = gen.generate();
            let dec = ulid.to_string();
            let ts = ulid.timestamp_ms();

            println!("{:<20} | {:<15} | {:<20}", dec, ulid, ts);
        }
        println!("----------------------------\n");
    }

    #[test]
    fn test_helper_methods() {
        let ts = 1_700_000_000_000u64; // Example timestamp
        let rand = 0xABCDu16; // Example random part

        let id = FastUlidHalfGen::from_parts(ts, rand);

        assert_eq!(FastUlidHalfGen::timestamp_ms(id), ts);
        assert_eq!(FastUlidHalfGen::random_part(id), rand);

        // Verify bit structure manually
        assert_eq!(id >> 16, ts);
        assert_eq!(id & 0xFFFF, rand as u64);
    }

    #[test]
    fn test_snowflake_gen() {
        let mut gen = SnowflakeGen::new(1); // Machine ID 1
        let mut last = gen.generate();

        println!("Snowflake ID: {}", last);

        for _ in 0..10000 {
            let next = gen.generate();
            assert!(next > last, "Snowflake IDs must be strictly increasing");

            // Check Machine ID part (bits 12-19)
            let machine_part = (next >> 12) & 0xFF;
            assert_eq!(machine_part, 1);

            last = next;
        }
    }

    #[test]
    fn test_snowflake_helpers() {
        let ts = 1_700_000_000_000u64; // Example timestamp
        let machine_id = 42u8; // Example machine ID
        let seq = 1234u16; // Example sequence

        let id = SnowflakeGen::from_parts(ts, machine_id, seq);

        assert_eq!(SnowflakeGen::timestamp_ms(id), ts);
        assert_eq!(SnowflakeGen::machine_id(id), machine_id);
        assert_eq!(SnowflakeGen::sequence(id), seq);

        // Verify bit structure manually
        assert_eq!(id >> 20, ts);
        assert_eq!((id >> 12) & 0xFF, machine_id as u64);
        assert_eq!(id & 0xFFF, seq as u64);
    }

    #[test]
    fn demo_usage_snowflake_gen() {
        let mut gen = SnowflakeGen::new(1); // Machine ID 1
        println!("\n--- SnowflakeGen Demo ---");
        println!(
            "{:<20} | {:<15} | {:<20}",
            "u64 (Decimal)", "Machine ID", "Timestamp (ms)"
        );
        println!("{:-<20}-+-{:-<15}-+-{:-<20}", "", "", "");

        for _ in 0..5 {
            let id = gen.generate();
            let machine_id = SnowflakeGen::machine_id(id);
            let ts = SnowflakeGen::timestamp_ms(id);

            println!("{:<20} | {:<15} | {:<20}", id, machine_id, ts);
        }
        println!("----------------------------\n");
    }
}
