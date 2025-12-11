// Enhanced error types for internal transfers
use std::fmt;

#[derive(Debug, Clone)]
pub enum TransferError {
    // Validation errors
    InvalidAmount(String),
    InvalidAsset(String),
    AssetMismatch { from: String, to: String },
    SameAccount,
    InvalidPrecision { value: String, expected: u32 },
    AmountTooSmall { amount: i64, minimum: i64 },

    // Permission errors
    PermissionDenied { user_id: u64, reason: String },

    // Balance errors
    InsufficientBalance { available: i64, required: i64, asset: String },

    // TigerBeetle errors
    TbConnectionError(String),
    TbOperationFailed { operation: String, reason: String },
    TbTransferNotFound(u128),

    // Database errors
    DbConnectionError(String),
    DbQueryFailed(String),
    DbRecordNotFound(i64),
    DbDuplicateKey(i64),

    // State errors
    InvalidStateTransition { from: String, to: String },
    TransferAlreadyCompleted(i64),
    TransferAlreadyFailed(i64),

    // System errors
    SystemOverloaded,
    ServiceUnavailable(String),
    Timeout(String),

    // Unknown
    Unknown(String),
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAmount(msg) => write!(f, "Invalid amount: {}", msg),
            Self::InvalidAsset(asset) => write!(f, "Invalid asset: {}", asset),
            Self::AssetMismatch { from, to } => {
                write!(f, "Asset mismatch: from {} to {}", from, to)
            }
            Self::SameAccount => write!(f, "Cannot transfer to same account"),
            Self::InvalidPrecision { value, expected } => {
                write!(f, "Invalid precision for {}, expected {} decimals", value, expected)
            }
            Self::AmountTooSmall { amount, minimum } => {
                write!(f, "Amount {} too small, minimum is {}", amount, minimum)
            }
            Self::PermissionDenied { user_id, reason } => {
                write!(f, "Permission denied for user {}: {}", user_id, reason)
            }
            Self::InsufficientBalance { available, required, asset } => {
                write!(
                    f,
                    "Insufficient balance: have {}, need {} {}",
                    available, required, asset
                )
            }
            Self::TbConnectionError(msg) => write!(f, "TigerBeetle connection error: {}", msg),
            Self::TbOperationFailed { operation, reason } => {
                write!(f, "TigerBeetle {} failed: {}", operation, reason)
            }
            Self::TbTransferNotFound(id) => write!(f, "Transfer {} not found in TigerBeetle", id),
            Self::DbConnectionError(msg) => write!(f, "Database connection error: {}", msg),
            Self::DbQueryFailed(msg) => write!(f, "Database query failed: {}", msg),
            Self::DbRecordNotFound(id) => write!(f, "Transfer {} not found", id),
            Self::DbDuplicateKey(id) => write!(f, "Transfer {} already exists", id),
            Self::InvalidStateTransition { from, to } => {
                write!(f, "Invalid state transition: {} -> {}", from, to)
            }
            Self::TransferAlreadyCompleted(id) => write!(f, "Transfer {} already completed", id),
            Self::TransferAlreadyFailed(id) => write!(f, "Transfer {} already failed", id),
            Self::SystemOverloaded => write!(f, "System overloaded, please try again later"),
            Self::ServiceUnavailable(service) => write!(f, "Service unavailable: {}", service),
            Self::Timeout(operation) => write!(f, "Operation timed out: {}", operation),
            Self::Unknown(msg) => write!(f, "Unknown error: {}", msg),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<anyhow::Error> for TransferError {
    fn from(err: anyhow::Error) -> Self {
        TransferError::Unknown(err.to_string())
    }
}

// Error code mapping for API responses
impl TransferError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidAmount(_) => "INVALID_AMOUNT",
            Self::InvalidAsset(_) => "INVALID_ASSET",
            Self::AssetMismatch { .. } => "ASSET_MISMATCH",
            Self::SameAccount => "SAME_ACCOUNT",
            Self::InvalidPrecision { .. } => "INVALID_PRECISION",
            Self::AmountTooSmall { .. } => "AMOUNT_TOO_SMALL",
            Self::PermissionDenied { .. } => "PERMISSION_DENIED",
            Self::InsufficientBalance { .. } => "INSUFFICIENT_BALANCE",
            Self::TbConnectionError(_) => "TB_CONNECTION_ERROR",
            Self::TbOperationFailed { .. } => "TB_OPERATION_FAILED",
            Self::TbTransferNotFound(_) => "TB_TRANSFER_NOT_FOUND",
            Self::DbConnectionError(_) => "DB_CONNECTION_ERROR",
            Self::DbQueryFailed(_) => "DB_QUERY_FAILED",
            Self::DbRecordNotFound(_) => "TRANSFER_NOT_FOUND",
            Self::DbDuplicateKey(_) => "DUPLICATE_TRANSFER",
            Self::InvalidStateTransition { .. } => "INVALID_STATE_TRANSITION",
            Self::TransferAlreadyCompleted(_) => "TRANSFER_COMPLETED",
            Self::TransferAlreadyFailed(_) => "TRANSFER_FAILED",
            Self::SystemOverloaded => "SYSTEM_OVERLOADED",
            Self::ServiceUnavailable(_) => "SERVICE_UNAVAILABLE",
            Self::Timeout(_) => "TIMEOUT",
            Self::Unknown(_) => "UNKNOWN_ERROR",
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::TbConnectionError(_)
                | Self::DbConnectionError(_)
                | Self::SystemOverloaded
                | Self::ServiceUnavailable(_)
                | Self::Timeout(_)
        )
    }

    pub fn is_user_error(&self) -> bool {
        matches!(
            self,
            Self::InvalidAmount(_)
                | Self::InvalidAsset(_)
                | Self::AssetMismatch { .. }
                | Self::SameAccount
                | Self::InvalidPrecision { .. }
                | Self::AmountTooSmall { .. }
                | Self::InsufficientBalance { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        let err = TransferError::InsufficientBalance {
            available: 100,
            required: 200,
            asset: "USDT".to_string(),
        };
        assert_eq!(err.error_code(), "INSUFFICIENT_BALANCE");
        assert!(!err.is_retryable());
        assert!(err.is_user_error());

        let err2 = TransferError::TbConnectionError("timeout".to_string());
        assert_eq!(err2.error_code(), "TB_CONNECTION_ERROR");
        assert!(err2.is_retryable());
        assert!(!err2.is_user_error());
    }

    #[test]
    fn test_error_display() {
        let err = TransferError::AssetMismatch {
            from: "BTC".to_string(),
            to: "USDT".to_string(),
        };
        assert_eq!(err.to_string(), "Asset mismatch: from BTC to USDT");
    }
}
