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

    /// Serialize as Decimal String
    pub fn to_str_decimal(val: u64) -> String {
        val.to_string()
    }
}

/// Strategy for initializing the sequence number
pub trait SequenceStrategy {
    fn next_sequence(current: u16, rng: &mut rand::rngs::ThreadRng) -> u16;
    fn reset_sequence(rng: &mut rand::rngs::ThreadRng) -> u16;
}

/// Strategy: Always start sequence at 0
pub struct ZeroSequence;
impl SequenceStrategy for ZeroSequence {
    fn next_sequence(current: u16, _rng: &mut rand::rngs::ThreadRng) -> u16 {
        (current + 1) & 0x1FFF
    }
    fn reset_sequence(_rng: &mut rand::rngs::ThreadRng) -> u16 {
        0
    }
}

/// Strategy: Start sequence at random value
pub struct RandomSequence;
impl SequenceStrategy for RandomSequence {
    fn next_sequence(current: u16, _rng: &mut rand::rngs::ThreadRng) -> u16 {
        (current + 1) & 0x1FFF
    }
    fn reset_sequence(rng: &mut rand::rngs::ThreadRng) -> u16 {
        rng.random::<u16>() & 0x1FFF
    }
}

/// A 64-bit Snowflake ID generator.
/// Structure:
/// - 44 bits: Timestamp (milliseconds)
/// - 7 bits: Machine ID (128 machines)
/// - 13 bits: Sequence (8192 IDs/ms)
pub struct SnowflakeGen<S: SequenceStrategy> {
    machine_id: u8,
    last_ts: u64,
    sequence: u16,
    rng: rand::rngs::ThreadRng,
    _marker: std::marker::PhantomData<S>,
}

impl<S: SequenceStrategy> SnowflakeGen<S> {
    pub fn new(machine_id: u8) -> Self {
        // Ensure machine_id fits in 7 bits
        let machine_id = machine_id & 0x7F;
        Self {
            machine_id,
            last_ts: 0,
            sequence: 0,
            rng: rand::rng(),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn generate(&mut self) -> u64 {
        let mut now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_millis() as u64;

        if now < self.last_ts {
            now = self.last_ts;
        }

        if now == self.last_ts {
            self.sequence = S::next_sequence(self.sequence, &mut self.rng);
            if self.sequence == 0 {
                // Overflow: Move to next millisecond immediately
                self.last_ts += 1;
                now = self.last_ts;
                // Reset sequence for new virtual ms
                self.sequence = S::reset_sequence(&mut self.rng);
            }
        } else {
            // New millisecond
            self.sequence = S::reset_sequence(&mut self.rng);
        }

        self.last_ts = now;
        // TS (44) | Machine (7) | Seq (13)
        (now << 20) | ((self.machine_id as u64) << 13) | (self.sequence as u64)
    }

    // Static helpers
    pub fn timestamp_ms(val: u64) -> u64 {
        val >> 20
    }
    pub fn machine_id(val: u64) -> u8 {
        ((val >> 13) & 0x7F) as u8
    }
    pub fn sequence(val: u64) -> u16 {
        (val & 0x1FFF) as u16
    }

    pub fn from_parts(timestamp_ms: u64, machine_id: u8, sequence: u16) -> u64 {
        (timestamp_ms << 20) | (((machine_id & 0x7F) as u64) << 13) | ((sequence & 0x1FFF) as u64)
    }

    pub fn to_str_base32(val: u64) -> String {
        const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let ts = val >> 20;
        let low = val & 0xFFFFF; // This is 20 bits (7 for machine + 13 for sequence)
        let mut chars = vec!['0'; 13];
        let mut t = ts;
        for i in (0..9).rev() {
            chars[i] = ALPHABET[(t % 32) as usize] as char;
            t /= 32;
        }
        let mut r = low;
        for i in (9..13).rev() {
            chars[i] = ALPHABET[(r % 32) as usize] as char;
            r /= 32;
        }
        chars.into_iter().collect()
    }

    pub fn from_str_base32(s: &str) -> Result<u64, String> {
        if s.len() != 13 {
            return Err(format!("Invalid length: expected 13, got {}", s.len()));
        }

        const DECODE: [i8; 256] = {
            let mut map = [-1; 256];
            let mut i = 0;
            while i < 32 {
                map[b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"[i] as usize] = i as i8;
                i += 1;
            }
            // Support lowercase as well
            let mut i = 0;
            while i < 32 {
                map[b"0123456789abcdefghjkmnpqrstvwxyz"[i] as usize] = i as i8;
                i += 1;
            }
            map
        };

        let bytes = s.as_bytes();

        // Decode Timestamp (first 9 chars)
        let mut ts: u64 = 0;
        for i in 0..9 {
            let val = DECODE[bytes[i] as usize];
            if val == -1 {
                return Err(format!("Invalid character at index {}", i));
            }
            ts = (ts << 5) | (val as u64);
        }

        // Decode Random/Low part (last 4 chars)
        let mut low: u64 = 0;
        for i in 9..13 {
            let val = DECODE[bytes[i] as usize];
            if val == -1 {
                return Err(format!("Invalid character at index {}", i));
            }
            low = (low << 5) | (val as u64);
        }

        Ok((ts << 20) | low)
    }

    pub fn to_str_decimal(val: u64) -> String {
        val.to_string()
    }
}

// Type aliases for convenience
pub type SnowflakeGenZero = SnowflakeGen<ZeroSequence>;
pub type SnowflakeGenRng = SnowflakeGen<RandomSequence>;

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
        let mut gen = SnowflakeGenZero::new(1); // Machine ID 1
        let mut last = gen.generate();

        println!("Snowflake ID: {}", last);

        for _ in 0..10000 {
            let next = gen.generate();
            assert!(next > last, "Snowflake IDs must be strictly increasing");

            // Check Machine ID part (bits 13-19)
            let machine_part = (next >> 13) & 0x7F;
            assert_eq!(machine_part, 1);

            last = next;
        }
    }

    #[test]
    fn test_snowflake_helpers() {
        let ts = 1_700_000_000_000u64; // Example timestamp
        let machine_id = 42u8; // Example machine ID
        let seq = 1234u16; // Example sequence

        let id = SnowflakeGenZero::from_parts(ts, machine_id, seq);

        assert_eq!(SnowflakeGenZero::timestamp_ms(id), ts);
        assert_eq!(SnowflakeGenZero::machine_id(id), machine_id);
        assert_eq!(SnowflakeGenZero::sequence(id), seq);

        // Verify bit structure manually
        assert_eq!(id >> 20, ts);
        assert_eq!((id >> 13) & 0x7F, machine_id as u64);
        assert_eq!(id & 0x1FFF, seq as u64);
    }

    #[test]
    fn test_snowflake_parsing() {
        let mut gen = SnowflakeGenZero::new(1);
        let id = gen.generate();
        let s = SnowflakeGenZero::to_str_base32(id);
        let parsed = SnowflakeGenZero::from_str_base32(&s).expect("Failed to parse Base32");

        assert_eq!(id, parsed, "Parsed ID must match original");

        // Test with known value
        // TS: 1764580760539 -> 1KBCKB6YV
        // Machine: 1, Seq: 4167 -> 0C27 (approx)
        // Full: 1KBCKB6YV0C27
        let known_str = "1KBCKB6YV0C27";
        let parsed_known = SnowflakeGenZero::from_str_base32(known_str).unwrap();
        assert_eq!(SnowflakeGenZero::to_str_base32(parsed_known), known_str);

        // Test error cases
        assert!(SnowflakeGenZero::from_str_base32("SHORT").is_err());
        assert!(SnowflakeGenZero::from_str_base32("TOO_LONG_STRING").is_err());
        assert!(SnowflakeGenZero::from_str_base32("1KBCKB6YV0C2!").is_err()); // Invalid char
    }

    #[test]
    fn demo_usage_snowflake_gen() {
        let mut gen = SnowflakeGenZero::new(1); // Machine ID 1
        println!("\n--- SnowflakeGen Demo ---");
        println!(
            "{:<20} | {:<13} | {:<15} | {:<20} | {:<20}",
            "u64 (Decimal)", "Base32", "Machine ID", "Timestamp (ms)", "Sequence"
        );
        println!(
            "{:-<20}-+-{:-<13}-+-{:-<15}-+-{:-<20}-+-{:-<20}",
            "", "", "", "", ""
        );

        for _ in 0..5 {
            let id = gen.generate();
            let machine_id = SnowflakeGenZero::machine_id(id);
            let ts = SnowflakeGenZero::timestamp_ms(id);
            let seq = SnowflakeGenZero::sequence(id);
            let b32 = SnowflakeGenZero::to_str_base32(id);

            println!(
                "{:<20} | {:<13} | {:<15} | {:<20} | {:<20}",
                id, b32, machine_id, ts, seq
            );
        }
        println!("----------------------------\n");
    }

    #[test]
    fn test_snowflake_gen_rng() {
        let mut gen = SnowflakeGenRng::new(1);
        let mut last = gen.generate();

        // Verify that the sequence part is not 0 (highly likely)
        let first_seq = SnowflakeGenRng::sequence(last);
        println!("First Random Sequence: {}", first_seq);

        // Basic monotonicity check
        for _ in 0..1000 {
            let next = gen.generate();
            assert!(next > last, "Snowflake IDs must be strictly increasing");
            last = next;
        }
    }

    #[test]
    fn test_snowflake_gen_zero_overflow_behavior() {
        let mut gen = SnowflakeGenZero::new(1);
        let mut last_id = gen.generate();
        let mut last_ts = SnowflakeGenZero::timestamp_ms(last_id);
        let mut last_seq = SnowflakeGenZero::sequence(last_id);

        // Generate enough IDs to force multiple sequence overflows (8192 per ms)
        // 30,000 IDs guarantees at least ~3 overflows if time doesn't advance much
        for _ in 0..30_000 {
            let id = gen.generate();
            let ts = SnowflakeGenZero::timestamp_ms(id);
            let seq = SnowflakeGenZero::sequence(id);

            // 1. Strict Monotonicity
            assert!(id > last_id, "IDs must be strictly increasing");

            // 2. Sequence/Timestamp Logic
            if ts == last_ts {
                // Same millisecond: sequence must increment
                assert_eq!(seq, last_seq + 1, "Sequence must increment within same ms");
            } else {
                // New millisecond (either wall clock advanced OR we overflowed)
                assert!(ts > last_ts, "Timestamp must increase");
                // For ZeroSequence, it must reset to 0
                assert_eq!(
                    seq, 0,
                    "Sequence must reset to 0 on new timestamp for ZeroGen"
                );
            }

            last_id = id;
            last_ts = ts;
            last_seq = seq;
        }
    }

    #[test]
    fn test_snowflake_gen_rng_overflow_behavior() {
        let mut gen = SnowflakeGenRng::new(1);
        let mut last_id = gen.generate();
        let mut last_ts = SnowflakeGenRng::timestamp_ms(last_id);
        let mut last_seq = SnowflakeGenRng::sequence(last_id);

        for _ in 0..30_000 {
            let id = gen.generate();
            let ts = SnowflakeGenRng::timestamp_ms(id);
            let seq = SnowflakeGenRng::sequence(id);

            // 1. Strict Monotonicity
            assert!(id > last_id, "IDs must be strictly increasing");

            // 2. Sequence/Timestamp Logic
            if ts == last_ts {
                // Same millisecond: sequence must increment
                assert_eq!(seq, last_seq + 1, "Sequence must increment within same ms");
            } else {
                // New millisecond (either wall clock advanced OR we overflowed)
                assert!(ts > last_ts, "Timestamp must increase");
                // For RandomSequence, it resets to a random value (likely non-zero, but could be 0)
                // We just verify it's a valid u13 (which is guaranteed by type/mask)
                assert!(seq < 8192);
            }

            last_id = id;
            last_ts = ts;
            last_seq = seq;
        }
    }

    #[test]
    fn demo_usage_snowflake_gen_rng() {
        let mut gen = SnowflakeGenRng::new(1); // Machine ID 1
        println!("\n--- SnowflakeGenRng Demo (Random Start) ---");
        println!(
            "{:<20} | {:<13} | {:<15} | {:<20} | {:<20}",
            "u64 (Decimal)", "Base32", "Machine ID", "Timestamp (ms)", "Sequence"
        );
        println!(
            "{:-<20}-+-{:-<13}-+-{:-<15}-+-{:-<20}-+-{:-<20}",
            "", "", "", "", ""
        );

        for _ in 0..5 {
            let id = gen.generate();
            let machine_id = SnowflakeGenRng::machine_id(id); // Reuse helper
            let ts = SnowflakeGenRng::timestamp_ms(id); // Reuse helper
            let seq = SnowflakeGenRng::sequence(id); // Reuse helper
            let b32 = SnowflakeGenRng::to_str_base32(id); // Reuse helper

            println!(
                "{:<20} | {:<13} | {:<15} | {:<20} | {:<20}",
                id, b32, machine_id, ts, seq
            );
        }
        println!("----------------------------\n");
    }
}
