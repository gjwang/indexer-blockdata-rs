use crate::models::api_response::ApiResponse;
use crate::models::internal_transfer_types::{
    AccountType, InternalTransferData, InternalTransferRequest, TransferStatus,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// API Error response
#[derive(Debug, Serialize, Deserialize)]
pub struct TransferError {
    pub code: String,
    pub message: String,
}

/// Error codes
pub mod error_codes {
    pub const INVALID_AMOUNT: &str = "INVALID_AMOUNT";
    pub const INVALID_ASSET: &str = "INVALID_ASSET";
    pub const ASSET_MISMATCH: &str = "ASSET_MISMATCH";
    pub const SAME_ACCOUNT: &str = "SAME_ACCOUNT";
    pub const INSUFFICIENT_BALANCE: &str = "INSUFFICIENT_BALANCE";
    pub const PERMISSION_DENIED: &str = "PERMISSION_DENIED";
    pub const INVALID_PRECISION: &str = "INVALID_PRECISION";
}

/// Create success response
pub fn success_response(data: InternalTransferData) -> ApiResponse<Option<InternalTransferData>> {
    ApiResponse {
        status: 0,
        msg: "ok".to_string(),
        data: Some(data),
    }
}

/// Create error response
pub fn error_response(code: &str, message: String) -> ApiResponse<Option<InternalTransferData>> {
    ApiResponse {
        status: -1,
        msg: format!("{}: {}", code, message),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_response() {
        let data = InternalTransferData {
            request_id: "123".to_string(),
            from_account: AccountType::Funding {
                asset: "USDT".to_string(),
                user_id: 0,
            },
            to_account: AccountType::Spot {
                user_id: 100,
                asset: "USDT".to_string(),
            },
            amount: "1.00".to_string(),
            status: TransferStatus::Pending,
            created_at: 1702345678000,
        };

        let response = success_response(data);
        assert_eq!(response.status, 0);
        assert!(response.data.is_some());
    }

    #[test]
    fn test_error_response() {
        let response = error_response(
            error_codes::INVALID_AMOUNT,
            "Amount must be positive".to_string(),
        );
        assert_eq!(response.status, -1);
        assert!(response.data.is_none());
    }
}
