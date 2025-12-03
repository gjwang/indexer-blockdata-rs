use serde::{Deserialize, Serialize};

/// Simplified balance requests for internal account transfers
/// All transfers are between funding_account and user trading accounts
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum BalanceRequest {
    /// Transfer funds from funding_account to user's trading account (deposit)
    Deposit {
        request_id: String,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        timestamp: u64, // Unix timestamp in milliseconds
    },
    /// Transfer funds from user's trading account to funding_account (withdraw)
    Withdraw {
        request_id: String,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        timestamp: u64, // Unix timestamp in milliseconds
    },
}

impl BalanceRequest {
    pub fn request_id(&self) -> &str {
        match self {
            BalanceRequest::Deposit { request_id, .. } => request_id,
            BalanceRequest::Withdraw { request_id, .. } => request_id,
        }
    }

    pub fn user_id(&self) -> u64 {
        match self {
            BalanceRequest::Deposit { user_id, .. } => *user_id,
            BalanceRequest::Withdraw { user_id, .. } => *user_id,
        }
    }

    pub fn timestamp(&self) -> u64 {
        match self {
            BalanceRequest::Deposit { timestamp, .. } => *timestamp,
            BalanceRequest::Withdraw { timestamp, .. } => *timestamp,
        }
    }

    /// Check if request is within valid time window (60 seconds)
    pub fn is_within_time_window(&self, current_time_ms: u64) -> bool {
        const TIME_WINDOW_MS: u64 = 60_000; // 60 seconds
        let request_time = self.timestamp();
        
        if current_time_ms < request_time {
            // Request from future, reject
            return false;
        }
        
        let age_ms = current_time_ms - request_time;
        age_ms <= TIME_WINDOW_MS
    }
}

// Remove old complex enums
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum DepositSource {
    Blockchain {
        chain: String,
        required_confirmations: u32,
    },
    BankTransfer {
        reference: String,
    },
    Internal {
        from_user_id: u64,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum WithdrawDestination {
    Blockchain { chain: String, address: String },
    BankTransfer { account: String },
    Internal { to_user_id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_window_validation() {
        let current_time = 1000000;
        
        // Valid: within 60 seconds
        let req = BalanceRequest::Deposit {
            request_id: "test1".to_string(),
            user_id: 1,
            asset_id: 1,
            amount: 100,
            timestamp: current_time - 30_000, // 30 seconds ago
        };
        assert!(req.is_within_time_window(current_time));

        // Invalid: too old (>60 seconds)
        let req = BalanceRequest::Deposit {
            request_id: "test2".to_string(),
            user_id: 1,
            asset_id: 1,
            amount: 100,
            timestamp: current_time - 70_000, // 70 seconds ago
        };
        assert!(!req.is_within_time_window(current_time));

        // Invalid: from future
        let req = BalanceRequest::Deposit {
            request_id: "test3".to_string(),
            user_id: 1,
            asset_id: 1,
            amount: 100,
            timestamp: current_time + 10_000, // 10 seconds in future
        };
        assert!(!req.is_within_time_window(current_time));
    }
}
