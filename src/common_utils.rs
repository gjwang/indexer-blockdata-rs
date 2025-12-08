use chrono::Utc;

/// Get current date as days since Unix epoch (UTC)
pub fn get_current_date() -> i32 {
    let now_ts = Utc::now().timestamp();
    (now_ts / 86400) as i32
}

/// Get current timestamp in milliseconds (UTC)
pub fn get_current_timestamp_ms() -> i64 {
    Utc::now().timestamp_millis()
}

// ============================================================================
// CRC32 Utilities
// ============================================================================

/// Compute CRC32 of a single byte slice (simple case)
#[inline]
pub fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Compute CRC32 of multiple byte slices
#[inline]
pub fn crc32_multi(slices: &[&[u8]]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    for slice in slices {
        hasher.update(slice);
    }
    hasher.finalize()
}

/// Verify CRC32 checksum
#[inline]
pub fn crc32_verify(data: &[u8], expected: u32) -> bool {
    crc32fast::hash(data) == expected
}

/// Verify CRC32 checksum of multiple slices
#[inline]
pub fn crc32_verify_multi(slices: &[&[u8]], expected: u32) -> bool {
    crc32_multi(slices) == expected
}

// ============================================================================
// WAL Record Format with Version
// ============================================================================
//
// Record Format: [VerLen: u32][CRC32: u32][Data: bytes]
// - VerLen: [Version(8-bit)][Length(24-bit)] packed in u32
//   - High byte (bits 24-31): Version (0-255)
//   - Low 3 bytes (bits 0-23): Data length (max 16MB)
// - CRC32: hash(verlen_bytes + data)
// - Data: payload bytes
//
// Byte layout (Big-Endian):
//   [Ver][Len_hi][Len_mid][Len_lo][CRC3][CRC2][CRC1][CRC0][Data...]
//   0    1       2        3       4     5     6     7     8+

/// WAL record header size: VerLen(4) + CRC(4) = 8 bytes
pub const WAL_RECORD_HEADER_SIZE: usize = 8;

/// Maximum data length: 24 bits = 16MB
pub const WAL_RECORD_MAX_LEN: usize = 0x00FF_FFFF; // 16,777,215 bytes

/// Current WAL schema version
/// - Version 0: Original format (current)
/// - Version 1+: Reserved for future schema changes
pub const WAL_SCHEMA_VERSION: u8 = 0;

/// Pack version + length into u32 (Big-Endian layout)
/// to_be_bytes() gives: [version, len_high, len_mid, len_low]
#[inline]
fn pack_verlen(version: u8, length: usize) -> u32 {
    debug_assert!(length <= WAL_RECORD_MAX_LEN, "WAL record too large");
    ((version as u32) << 24) | (length as u32 & WAL_RECORD_MAX_LEN as u32)
}

/// Unpack version + length from u32
#[inline]
fn unpack_verlen(verlen: u32) -> (u8, usize) {
    let version = (verlen >> 24) as u8;
    let length = (verlen & WAL_RECORD_MAX_LEN as u32) as usize;
    (version, length)
}

/// Serialize data into WAL record format: [Ver(1)][Len(3)][CRC(4)][Data]
/// Uses current WAL_SCHEMA_VERSION.
pub fn wal_record_serialize(data: &[u8]) -> Vec<u8> {
    wal_record_serialize_v(WAL_SCHEMA_VERSION, data)
}

/// Serialize data with explicit version
/// Format: [Version(1)][Len(3)][CRC(4)][Data] (Big-Endian header)
pub fn wal_record_serialize_v(version: u8, data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= WAL_RECORD_MAX_LEN, "WAL record too large: {} > {}", data.len(), WAL_RECORD_MAX_LEN);

    let verlen = pack_verlen(version, data.len());
    let verlen_bytes = verlen.to_be_bytes();  // Big-Endian

    // CRC includes verlen + data
    let crc = crc32_multi(&[&verlen_bytes, data]);
    let crc_bytes = crc.to_be_bytes();  // Big-Endian for consistency

    let mut buf = Vec::with_capacity(8 + data.len());
    buf.extend_from_slice(&verlen_bytes);
    buf.extend_from_slice(&crc_bytes);
    buf.extend_from_slice(data);
    buf
}

/// Deserialize WAL record, returns (version, data, bytes_consumed) or error
pub fn wal_record_deserialize_v(buf: &[u8]) -> Result<(u8, &[u8], usize), WalRecordError> {
    if buf.len() < WAL_RECORD_HEADER_SIZE {
        return Err(WalRecordError::TooShort);
    }

    let verlen_bytes: [u8; 4] = buf[0..4].try_into().unwrap();
    let verlen = u32::from_be_bytes(verlen_bytes);  // Big-Endian
    let (version, data_len) = unpack_verlen(verlen);

    if data_len == 0 {
        return Err(WalRecordError::ZeroLength);
    }

    if data_len > WAL_RECORD_MAX_LEN {
        return Err(WalRecordError::LengthOverflow(data_len));
    }

    let stored_crc = u32::from_be_bytes(buf[4..8].try_into().unwrap());  // Big-Endian

    let total_len = WAL_RECORD_HEADER_SIZE + data_len;
    if buf.len() < total_len {
        return Err(WalRecordError::TooShort);
    }

    let data = &buf[8..total_len];

    // Verify CRC (verlen_bytes + data)
    let computed_crc = crc32_multi(&[&verlen_bytes, data]);
    if computed_crc != stored_crc {
        return Err(WalRecordError::CrcMismatch { stored: stored_crc, computed: computed_crc });
    }

    Ok((version, data, total_len))
}

/// Deserialize WAL record (ignores version), returns (data, bytes_consumed) or error
/// For backward compatibility with existing code.
pub fn wal_record_deserialize(buf: &[u8]) -> Result<(&[u8], usize), WalRecordError> {
    let (_version, data, consumed) = wal_record_deserialize_v(buf)?;
    Ok((data, consumed))
}

/// WAL record errors
#[derive(Debug, Clone, PartialEq)]
pub enum WalRecordError {
    TooShort,
    ZeroLength,
    LengthOverflow(usize),
    CrcMismatch { stored: u32, computed: u32 },
    UnsupportedVersion(u8),
}

impl std::fmt::Display for WalRecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalRecordError::TooShort => write!(f, "Buffer too short for WAL record"),
            WalRecordError::ZeroLength => write!(f, "Zero length WAL record"),
            WalRecordError::LengthOverflow(len) => {
                write!(f, "WAL record length overflow: {} > {}", len, WAL_RECORD_MAX_LEN)
            }
            WalRecordError::CrcMismatch { stored, computed } => {
                write!(f, "CRC mismatch: stored={}, computed={}", stored, computed)
            }
            WalRecordError::UnsupportedVersion(v) => write!(f, "Unsupported WAL version: {}", v),
        }
    }
}

impl std::error::Error for WalRecordError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_record_roundtrip() {
        let data = b"hello world";
        let record = wal_record_serialize(data);

        let (parsed, consumed) = wal_record_deserialize(&record).unwrap();
        assert_eq!(parsed, data);
        assert_eq!(consumed, record.len());
    }

    #[test]
    fn test_wal_record_version() {
        let data = b"test data";
        let record = wal_record_serialize(data);

        // Check version is embedded
        let (version, parsed, _) = wal_record_deserialize_v(&record).unwrap();
        assert_eq!(version, WAL_SCHEMA_VERSION);
        assert_eq!(parsed, data);
    }

    #[test]
    fn test_wal_record_explicit_version() {
        let data = b"test data";
        let record = wal_record_serialize_v(42, data);

        let (version, parsed, _) = wal_record_deserialize_v(&record).unwrap();
        assert_eq!(version, 42);
        assert_eq!(parsed, data);
    }

    #[test]
    fn test_pack_unpack_verlen() {
        // Test edge cases
        let test_cases = [
            (0u8, 0usize),
            (1, 100),
            (255, WAL_RECORD_MAX_LEN),
            (1, WAL_RECORD_MAX_LEN),
        ];

        for (version, length) in test_cases {
            let packed = pack_verlen(version, length);
            let (v, l) = unpack_verlen(packed);
            assert_eq!(v, version, "version mismatch");
            assert_eq!(l, length, "length mismatch");
        }
    }

    #[test]
    fn test_wal_record_crc_includes_data() {
        let data = b"test data";
        let record = wal_record_serialize(data);

        // Corrupt the data
        let mut corrupted = record.clone();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xFF;

        // Should fail CRC check
        assert!(matches!(
            wal_record_deserialize(&corrupted),
            Err(WalRecordError::CrcMismatch { .. })
        ));
    }

    #[test]
    fn test_wal_record_too_short() {
        assert!(matches!(
            wal_record_deserialize(&[1, 2, 3]),
            Err(WalRecordError::TooShort)
        ));
    }
}

