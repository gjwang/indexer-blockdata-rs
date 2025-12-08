//! WAL entry format
//!
//! Format: [Length: u32][CRC32: u32][Type: u8][Payload: bytes]
//!
//! - Length: total bytes including header (4 + 4 + 1 + payload)
//! - CRC32: checksum of Type + Payload
//! - Type: entry type discriminator
//! - Payload: serialized data

use crc32fast::Hasher;

/// Header size: length(4) + crc32(4) + type(1) = 9 bytes
pub const HEADER_SIZE: usize = 9;

/// Entry types for WAL
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WalEntryType {
    /// Deposit funds
    Deposit = 1,
    /// Withdraw funds
    Withdraw = 2,
    /// Order placed (funds locked)
    OrderLock = 3,
    /// Order cancelled (funds unlocked)
    OrderUnlock = 4,
    /// Trade executed
    Trade = 5,
    /// Fee charged
    Fee = 6,
    /// Snapshot marker
    Snapshot = 7,
}

impl TryFrom<u8> for WalEntryType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(WalEntryType::Deposit),
            2 => Ok(WalEntryType::Withdraw),
            3 => Ok(WalEntryType::OrderLock),
            4 => Ok(WalEntryType::OrderUnlock),
            5 => Ok(WalEntryType::Trade),
            6 => Ok(WalEntryType::Fee),
            7 => Ok(WalEntryType::Snapshot),
            _ => Err(value),
        }
    }
}

/// WAL entry representation
#[derive(Debug, Clone)]
pub struct WalEntry {
    pub entry_type: WalEntryType,
    pub payload: Vec<u8>,
}

impl WalEntry {
    /// Create a new entry
    pub fn new(entry_type: WalEntryType, payload: Vec<u8>) -> Self {
        Self { entry_type, payload }
    }

    /// Serialize entry to bytes (including header)
    pub fn serialize(&self) -> Vec<u8> {
        let total_len = HEADER_SIZE + self.payload.len();
        let mut buf = Vec::with_capacity(total_len);

        // Calculate CRC32 of type + payload
        let mut hasher = Hasher::new();
        hasher.update(&[self.entry_type as u8]);
        hasher.update(&self.payload);
        let crc = hasher.finalize();

        // Write header
        buf.extend_from_slice(&(total_len as u32).to_le_bytes()); // Length
        buf.extend_from_slice(&crc.to_le_bytes()); // CRC32
        buf.push(self.entry_type as u8); // Type

        // Write payload
        buf.extend_from_slice(&self.payload);

        buf
    }

    /// Deserialize entry from bytes
    /// Returns (entry, bytes_consumed) or error
    pub fn deserialize(data: &[u8]) -> Result<(Self, usize), WalError> {
        if data.len() < HEADER_SIZE {
            return Err(WalError::TooShort);
        }

        // Read header
        let len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let stored_crc = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let type_byte = data[8];

        // Validate length
        if len < HEADER_SIZE {
            return Err(WalError::InvalidLength(len));
        }
        if data.len() < len {
            return Err(WalError::TooShort);
        }

        // Parse type
        let entry_type =
            WalEntryType::try_from(type_byte).map_err(|_| WalError::InvalidType(type_byte))?;

        // Extract payload
        let payload = data[HEADER_SIZE..len].to_vec();

        // Verify CRC
        let mut hasher = Hasher::new();
        hasher.update(&[type_byte]);
        hasher.update(&payload);
        let computed_crc = hasher.finalize();

        if computed_crc != stored_crc {
            return Err(WalError::CrcMismatch { stored: stored_crc, computed: computed_crc });
        }

        Ok((Self { entry_type, payload }, len))
    }
}

/// WAL errors
#[derive(Debug, Clone, PartialEq)]
pub enum WalError {
    TooShort,
    InvalidLength(usize),
    InvalidType(u8),
    CrcMismatch { stored: u32, computed: u32 },
    IoError(String),
}

impl std::fmt::Display for WalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalError::TooShort => write!(f, "Data too short for WAL entry"),
            WalError::InvalidLength(len) => write!(f, "Invalid entry length: {}", len),
            WalError::InvalidType(t) => write!(f, "Invalid entry type: {}", t),
            WalError::CrcMismatch { stored, computed } => {
                write!(f, "CRC mismatch: stored={}, computed={}", stored, computed)
            }
            WalError::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for WalError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_roundtrip() {
        let entry = WalEntry::new(WalEntryType::Deposit, vec![1, 2, 3, 4, 5]);
        let serialized = entry.serialize();
        let (deserialized, consumed) = WalEntry::deserialize(&serialized).unwrap();

        assert_eq!(consumed, serialized.len());
        assert_eq!(deserialized.entry_type, WalEntryType::Deposit);
        assert_eq!(deserialized.payload, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_entry_types() {
        for (type_val, expected) in [
            (1, WalEntryType::Deposit),
            (2, WalEntryType::Withdraw),
            (3, WalEntryType::OrderLock),
            (4, WalEntryType::OrderUnlock),
            (5, WalEntryType::Trade),
            (6, WalEntryType::Fee),
            (7, WalEntryType::Snapshot),
        ] {
            assert_eq!(WalEntryType::try_from(type_val), Ok(expected));
        }
    }

    #[test]
    fn test_invalid_type() {
        assert!(WalEntryType::try_from(0).is_err());
        assert!(WalEntryType::try_from(100).is_err());
    }

    #[test]
    fn test_crc_validation() {
        let entry = WalEntry::new(WalEntryType::Trade, vec![10, 20, 30]);
        let mut serialized = entry.serialize();

        // Corrupt the payload
        let last = serialized.len() - 1;
        serialized[last] ^= 0xFF;

        let result = WalEntry::deserialize(&serialized);
        assert!(matches!(result, Err(WalError::CrcMismatch { .. })));
    }

    #[test]
    fn test_too_short() {
        let result = WalEntry::deserialize(&[1, 2, 3]);
        assert!(matches!(result, Err(WalError::TooShort)));
    }

    #[test]
    fn test_empty_payload() {
        let entry = WalEntry::new(WalEntryType::Snapshot, vec![]);
        let serialized = entry.serialize();
        let (deserialized, _) = WalEntry::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.entry_type, WalEntryType::Snapshot);
        assert!(deserialized.payload.is_empty());
    }
}
