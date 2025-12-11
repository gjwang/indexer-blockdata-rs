use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnlockReason {
    Cancel,
    Expire,
    FillRemainder, // Unused funds from a partial fill or price improvement
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BalanceEvent {
    /// External deposit into the exchange
    /// Flow: Omnibus -> User
    Deposited {
        tx_id: u64,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    },

    /// External withdrawal from the exchange
    /// Flow: User -> Omnibus
    Withdrawn {
        tx_id: u64,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    },

    /// Funds locked for an open order
    /// Flow: User -> Holding (Pending)
    FundsLocked {
        order_id: u64,
        user_id: u64,
        asset_id: u32,
        amount: u64, // Needs to include Max Potential Fee
    },

    /// Funds unlocked
    /// Flow: Void Pending (Return to User)
    FundsUnlocked {
        order_id: u64,
        reason: UnlockReason,
    },

    /// Trade Settlement (Atomic Batch)
    /// Flow: Post Pending -> Swap -> Fees
    TradeSettled {
        match_id: u64,
        buyer_id: u64,
        seller_id: u64,
        base_asset: u32,
        quote_asset: u32,
        base_qty: u64,
        quote_amt: u64,
        buyer_fee: u64,
        seller_fee: u64,
        buyer_order_id: u64,
        seller_order_id: u64,
    },

    /// User Account Created (Lazy)
    AccountCreated {
        user_id: u64,
    },
}
