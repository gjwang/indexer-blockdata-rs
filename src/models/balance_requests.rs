use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum BalanceRequest {
    Deposit {
        request_id: String,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        source: DepositSource,
        external_tx_id: String,
        confirmations: u32,
    },
    Withdraw {
        request_id: String,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        destination: WithdrawDestination,
        external_address: String,
    },
    WithdrawConfirm {
        request_id: String,
        external_tx_id: String,
    },
    WithdrawReject {
        request_id: String,
        reason: String,
    },
}

impl BalanceRequest {
    pub fn request_id(&self) -> &str {
        match self {
            BalanceRequest::Deposit { request_id, .. } => request_id,
            BalanceRequest::Withdraw { request_id, .. } => request_id,
            BalanceRequest::WithdrawConfirm { request_id, .. } => request_id,
            BalanceRequest::WithdrawReject { request_id, .. } => request_id,
        }
    }

    pub fn user_id(&self) -> Option<u64> {
        match self {
            BalanceRequest::Deposit { user_id, .. } => Some(*user_id),
            BalanceRequest::Withdraw { user_id, .. } => Some(*user_id),
            _ => None,
        }
    }
}

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
