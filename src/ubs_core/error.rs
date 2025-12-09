//! Error types for UBSCore

/// Reasons for rejecting an order or balance operation
#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    // Order validation errors
    OrderTooOld,
    FutureTimestamp,
    DuplicateOrderId,
    InvalidSymbol,
    OrderCostOverflow,

    // Balance errors
    InsufficientBalance,
    InsufficientFunds,    // Not enough available balance to lock
    InsufficientFrozen,   // Not enough frozen balance to spend/unlock
    BalanceOverflow,      // Deposit would overflow u64
    AccountNotFound,

    // System errors
    SystemBusy,
    /// Internal error (WAL, serialization, etc.)
    InternalError,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reject_reason_debug() {
        let reason = RejectReason::InsufficientBalance;
        assert_eq!(format!("{:?}", reason), "InsufficientBalance");
    }

    #[test]
    fn test_reject_reason_clone() {
        let reason = RejectReason::SystemBusy;
        let cloned = reason.clone();
        assert_eq!(reason, cloned);
    }
}
