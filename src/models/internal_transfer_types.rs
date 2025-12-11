use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Account type for internal transfers
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "account_type", rename_all = "snake_case")]
pub enum AccountType {
    Funding {
        asset: String,
        #[serde(default)]
        user_id: u64, // Default to 0 for funding
    },
    Spot { user_id: u64, asset: String },
}

impl AccountType {
    pub fn asset(&self) -> String {
        match self {
            Self::Funding { asset, .. } => asset.to_uppercase(),
            Self::Spot { asset, .. } => asset.to_uppercase(),
        }
    }

    pub fn to_tb_account_id(&self, asset_id: u32) -> u128 {
        match self {
            Self::Funding { .. } => ((0_u64 as u128) << 64) | (asset_id as u128),
            Self::Spot { user_id, .. } => ((*user_id as u128) << 64) | (asset_id as u128),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Funding { .. } => "funding",
            Self::Spot { .. } => "spot",
        }
    }

    pub fn user_id(&self) -> Option<u64> {
        match self {
            Self::Funding { .. } => Some(0),
            Self::Spot { user_id, .. } => Some(*user_id),
        }
    }
}

/// Transfer status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Requesting,
    Pending,
    Success,
    Failed,
}

impl TransferStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requesting => "requesting",
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "requesting" => Some(Self::Requesting),
            "pending" => Some(Self::Pending),
            "success" => Some(Self::Success),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success | Self::Failed)
    }
}

/// Internal transfer request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalTransferRequest {
    pub from_account: AccountType,
    pub to_account: AccountType,
    pub amount: Decimal,
}

/// Internal transfer response data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalTransferData {
    pub request_id: String,
    pub from_account: AccountType,
    pub to_account: AccountType,
    pub amount: String,
    pub status: TransferStatus,
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_type_json() {
        let funding = AccountType::Funding { asset: "USDT".to_string(), user_id: 0 };
        let json = serde_json::to_string(&funding).unwrap();
        assert!(json.contains("account_type"));

        let spot = AccountType::Spot { user_id: 100, asset: "BTC".to_string() };
        let json2 = serde_json::to_string(&spot).unwrap();
        assert!(json2.contains("spot"));
    }

    #[test]
    fn test_status() {
        assert_eq!(TransferStatus::Pending.as_str(), "pending");
        assert!(TransferStatus::Success.is_terminal());
    }
}
