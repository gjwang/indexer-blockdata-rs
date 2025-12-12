//! Core types for Internal Transfer system
//!
//! This module defines the fundamental types used across the transfer system.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::transfer::state::TransferState;

/// Service identifier - represents source or target of a transfer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceId {
    Funding,
    Trading,
}

impl ServiceId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceId::Funding => "funding",
            ServiceId::Trading => "trading",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "funding" => Some(ServiceId::Funding),
            "trading" => Some(ServiceId::Trading),
            _ => None,
        }
    }
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
    /// Unique identifier (UUID v4)
    pub req_id: Uuid,
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
        assert_eq!(ServiceId::Funding.as_str(), "funding");
        assert_eq!(ServiceId::Trading.as_str(), "trading");

        assert_eq!(ServiceId::from_str("funding"), Some(ServiceId::Funding));
        assert_eq!(ServiceId::from_str("trading"), Some(ServiceId::Trading));
        assert_eq!(ServiceId::from_str("invalid"), None);
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
