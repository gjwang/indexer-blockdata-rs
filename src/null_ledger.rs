// Null Ledger - Stub implementation for ME refactoring
// ME no longer maintains balance state - this allows compilation
// during transition to UBSCore-only balance management

use crate::ledger::{Ledger, LedgerCommand};
use crate::user_account::UserAccount;
use std::collections::HashMap;

/// NullLedger - Does nothing for balance operations
/// Used during ME refactoring to remove balance state
pub struct NullLedger;

impl NullLedger {
    pub fn new() -> Self {
        NullLedger
    }
}

impl Ledger for NullLedger {
    fn apply(&mut self, _cmd: &LedgerCommand) -> Result<(), anyhow::Error> {
        // ME no longer applies balance commands - UBSCore handles this
        Ok(())
    }

    fn get_balance(&self, _user_id: u64, _asset_id: u32) -> u64 {
        // Balance checking is done by UBSCore - return dummy value
        u64::MAX // Effectively unlimited - UBSCore already validated
    }

    fn get_frozen(&self, _user_id: u64, _asset_id: u32) -> u64 {
        0 // ME doesn't track frozen funds
    }

    fn get_balance_version(&self, _user_id: u64, _asset_id: u32) -> u64 {
        0 // Version tracking is in UBSCore
    }
}
