//! Core types for Internal Transfer system
//!
//! This module defines the fundamental types used across the transfer system.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::fast_ulid::SnowflakeGenRng;
use crate::transfer::state::TransferState;

/// Request ID - a 64-bit Snowflake ID for transfer requests
///
/// Structure (u64):
/// - 44 bits: Timestamp (milliseconds since epoch)
/// - 7 bits: Machine ID (up to 128 machines)
/// - 13 bits: Sequence (8192 IDs per millisecond)
///
/// Benefits over UUID:
/// - 8 bytes vs 16 bytes (50% smaller)
/// - Time-sortable (natural ordering)
/// - Monotonically increasing
/// - Machine ID for distributed generation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    /// Create a new RequestId from raw u64
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw u64 value
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// Convert to u128 (for TigerBeetle compatibility)
    pub fn as_u128(&self) -> u128 {
        self.0 as u128
    }

    /// Get timestamp component (milliseconds since epoch)
    pub fn timestamp_ms(&self) -> u64 {
        SnowflakeGenRng::timestamp_ms(self.0)
    }

    /// Get machine ID component
    pub fn machine_id(&self) -> u8 {
        SnowflakeGenRng::machine_id(self.0)
    }

    /// Get sequence component
    pub fn sequence(&self) -> u16 {
        SnowflakeGenRng::sequence(self.0)
    }

    /// Convert to Base32 string (13 chars)
    pub fn to_base32(&self) -> String {
        SnowflakeGenRng::to_str_base32(self.0)
    }

    /// Parse from Base32 string
    pub fn from_base32(s: &str) -> Result<Self, String> {
        SnowflakeGenRng::from_str_base32(s).map(Self)
    }

    /// Parse from decimal string
    pub fn from_str(s: &str) -> Result<Self, String> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|e| format!("Invalid RequestId: {}", e))
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for RequestId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as string for JSON compatibility
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<u64>()
            .map(RequestId)
            .map_err(serde::de::Error::custom)
    }
}

/// Service identifier - represents source or target of a transfer
///
/// Uses strum for automatic String conversion:
/// - `service.as_ref()` -> &str "funding" (zero-alloc)
/// - `service.to_string()` -> String "funding"
/// - `ServiceId::from_str("funding")` -> Result<ServiceId>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum_macros::Display, strum_macros::EnumString, strum_macros::AsRefStr)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ServiceId {
    Funding,
    Trading,
}

/// Unified result from any service adapter operation
///
/// This enum represents the three possible outcomes of any service call:
/// - Success: Operation definitely completed successfully
/// - Failed: Operation definitely failed (business failure, e.g., insufficient funds)
/// - Pending: Operation in-flight or technical error (requires retry)
#[derive(Debug, Clone, PartialEq)]
pub enum OpResult {
    /// Operation definitely succeeded
    Success,
    /// Operation definitely failed (business failure only)
    /// Examples: "Insufficient funds", "Account frozen", "Invalid user"
    Failed(String),
    /// Operation in-flight or technical error (retry required)
    /// Examples: Timeout, HTTP 500, connection refused
    Pending,
}

/// Transfer record stored in database
#[derive(Debug, Clone)]
pub struct TransferRecord {
    /// Unique identifier (Snowflake ID)
    pub req_id: RequestId,
    /// Source service (where funds come from)
    pub source: ServiceId,
    /// Target service (where funds go to)
    pub target: ServiceId,
    /// User performing the transfer
    pub user_id: u64,
    /// Asset being transferred
    pub asset_id: u32,
    /// Amount in smallest unit (e.g., satoshi)
    pub amount: u64,
    /// Current FSM state
    pub state: TransferState,
    /// Creation timestamp (ms)
    pub created_at: i64,
    /// Last update timestamp (ms)
    pub updated_at: i64,
    /// Error message if failed
    pub error: Option<String>,
    /// Retry count
    pub retry_count: u32,
}

/// Request to create a transfer (type-safe version)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    /// Source service
    pub from: ServiceId,
    /// Target service
    pub to: ServiceId,
    /// User performing the transfer
    pub user_id: u64,
    /// Asset being transferred
    pub asset_id: u32,
    /// Amount in smallest unit (e.g., satoshi)
    pub amount: u64,
}

/// Response from transfer operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResponse {
    pub req_id: String,
    pub status: String,     // "committed", "pending", "failed", "rolled_back"
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_id_serialization() {
        assert_eq!(ServiceId::Funding.as_ref(), "funding");
        assert_eq!(ServiceId::Trading.as_ref(), "trading");

        assert_eq!("funding".parse::<ServiceId>().unwrap(), ServiceId::Funding);
        assert_eq!("trading".parse::<ServiceId>().unwrap(), ServiceId::Trading);
        assert!("invalid".parse::<ServiceId>().is_err());
    }

    #[test]
    fn test_op_result_clone() {
        let success = OpResult::Success;
        let cloned = success.clone();
        assert!(matches!(cloned, OpResult::Success));

        let failed = OpResult::Failed("error".to_string());
        let cloned = failed.clone();
        assert!(matches!(cloned, OpResult::Failed(e) if e == "error"));

        let pending = OpResult::Pending;
        let cloned = pending.clone();
        assert!(matches!(cloned, OpResult::Pending));
    }

    #[test]
    fn test_transfer_request_json() {
        let req = TransferRequest {
            from: ServiceId::Funding,
            to: ServiceId::Trading,
            user_id: 4001,
            asset_id: 1,
            amount: 1000000,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"from\":\"funding\""));
        assert!(json.contains("\"to\":\"trading\""));

        let parsed: TransferRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.from, ServiceId::Funding);
        assert_eq!(parsed.to, ServiceId::Trading);
        assert_eq!(parsed.user_id, 4001);
        assert_eq!(parsed.asset_id, 1);
        assert_eq!(parsed.amount, 1000000);
    }
}
