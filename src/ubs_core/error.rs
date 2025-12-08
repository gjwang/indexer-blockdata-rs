//! Error types for UBSCore

/// Reasons for rejecting an order
#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    OrderTooOld,
    FutureTimestamp,
    DuplicateOrderId,
    InsufficientBalance,
    AccountNotFound,
    InvalidSymbol,
    OrderCostOverflow,
    SystemBusy,
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
