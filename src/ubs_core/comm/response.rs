//! Response message format for UBSCore → Gateway
//!
//! Sent on responses channel after order validation.

/// Response message from UBSCore to Gateway
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ResponseMessage {
    pub order_id: u64,
    pub accepted: u8,      // 1 = accepted, 0 = rejected
    pub reason_code: u8,   // Rejection reason code (0 if accepted)
}

// Use trait for to_bytes/from_bytes
impl super::WireMessage for ResponseMessage {}

impl ResponseMessage {
    /// Create accept response
    pub fn accept(order_id: u64) -> Self {
        Self {
            order_id,
            accepted: 1,
            reason_code: 0,
        }
    }

    /// Create reject response
    pub fn reject(order_id: u64, reason_code: u8) -> Self {
        Self {
            order_id,
            accepted: 0,
            reason_code,
        }
    }

    /// Check if accepted
    pub fn is_accepted(&self) -> bool {
        self.accepted == 1
    }
}

/// Reason codes for rejection
pub mod reason_codes {
    pub const INSUFFICIENT_BALANCE: u8 = 1;
    pub const DUPLICATE_ORDER_ID: u8 = 2;
    pub const ORDER_TOO_OLD: u8 = 3;
    pub const FUTURE_TIMESTAMP: u8 = 4;
    pub const INVALID_SYMBOL: u8 = 5;
    pub const ORDER_COST_OVERFLOW: u8 = 6;
    pub const ACCOUNT_NOT_FOUND: u8 = 7;
    pub const SYSTEM_BUSY: u8 = 8;
    pub const INTERNAL_ERROR: u8 = 99;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let msg = ResponseMessage::accept(12345);
        let bytes = msg.to_bytes();
        let parsed = ResponseMessage::from_bytes(bytes).unwrap();
        assert_eq!(parsed.order_id, 12345);
        assert!(parsed.is_accepted());
    }

    #[test]
    fn test_reject() {
        let msg = ResponseMessage::reject(12345, reason_codes::INSUFFICIENT_BALANCE);
        assert!(!msg.is_accepted());
        assert_eq!(msg.reason_code, 1);
    }
}
