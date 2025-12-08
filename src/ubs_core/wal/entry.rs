//! WAL entry format
//!
//! Format: [Len(4)][CRC(4)][Type(1)][Payload]
//!
//! - Common layer (wal_record_*) handles [Len(4)][CRC(4)][Data]
//! - Data = [Type(1)][Payload]
//! - Version is at WAL file/snapshot level, not per-entry

use crate::common_utils::{wal_record_deserialize, wal_record_serialize, WalRecordError};

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

    /// Serialize entry to bytes
    /// Format: [Len(4)][CRC(4)][Type(1)][Payload]
    pub fn serialize(&self) -> Vec<u8> {
        // Data = Type(1) + Payload
        let mut data = Vec::with_capacity(1 + self.payload.len());
        data.push(self.entry_type as u8);
        data.extend_from_slice(&self.payload);

        // Common layer wraps as: [Len(4)][CRC(4)][Data]
        wal_record_serialize(&data)
    }

    /// Deserialize entry from bytes
    pub fn deserialize(buf: &[u8]) -> Result<(Self, usize), WalError> {
        // Common layer handles [Len(4)][CRC(4)][Data]
        let (data, consumed) = wal_record_deserialize(buf)?;

        if data.is_empty() {
            return Err(WalError::TooShort);
        }

        // Data = Type(1) + Payload
        let type_byte = data[0];
        let entry_type =
            WalEntryType::try_from(type_byte).map_err(|_| WalError::InvalidType(type_byte))?;
        let payload = data[1..].to_vec();

        Ok((Self { entry_type, payload }, consumed))
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

impl From<WalRecordError> for WalError {
    fn from(err: WalRecordError) -> Self {
        match err {
            WalRecordError::TooShort => WalError::TooShort,
            WalRecordError::ZeroLength => WalError::InvalidLength(0),
            WalRecordError::LengthOverflow(len) => WalError::InvalidLength(len),
            WalRecordError::CrcMismatch { stored, computed } => {
                WalError::CrcMismatch { stored, computed }
            }
            WalRecordError::UnsupportedVersion(v) => WalError::InvalidType(v),
        }
    }
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
