//! Transfer State Machine
//!
//! Defines the FSM states, events, and transition function for internal transfers.

use serde::{Deserialize, Serialize};

/// Transfer FSM states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Transfer created, waiting for processing
    Init,
    /// Called source service, waiting for result
    SourcePending,
    /// Source confirmed debit/freeze
    SourceDone,
    /// Called target service, waiting for result
    TargetPending,
    /// Target failed, rolling back source
    Compensating,
    /// Transfer complete ✅
    Committed,
    /// Transfer cancelled, source restored ❌
    RolledBack,
    /// Source failed, no changes made ❌
    Failed,
}

impl TransferState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransferState::Init => "init",
            TransferState::SourcePending => "source_pending",
            TransferState::SourceDone => "source_done",
            TransferState::TargetPending => "target_pending",
            TransferState::Compensating => "compensating",
            TransferState::Committed => "committed",
            TransferState::RolledBack => "rolled_back",
            TransferState::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "init" => Some(TransferState::Init),
            "source_pending" => Some(TransferState::SourcePending),
            "source_done" => Some(TransferState::SourceDone),
            "target_pending" => Some(TransferState::TargetPending),
            "compensating" => Some(TransferState::Compensating),
            "committed" => Some(TransferState::Committed),
            "rolled_back" => Some(TransferState::RolledBack),
            "failed" => Some(TransferState::Failed),
            _ => None,
        }
    }

    /// Check if this is a terminal state (no further transitions possible)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TransferState::Committed | TransferState::RolledBack | TransferState::Failed
        )
    }

    /// Check if this state needs processing by scanner
    pub fn needs_processing(&self) -> bool {
        !self.is_terminal()
    }
}

/// FSM Events (inputs that trigger state transitions)
#[derive(Debug, Clone)]
pub enum TransferEvent {
    /// Called source, got Pending
    SourceCall,
    /// Source returned Success
    SourceOk,
    /// Source returned Failed
    SourceFail,
    /// Called target, got Pending
    TargetCall,
    /// Target returned Success
    TargetOk,
    /// Target returned Failed
    TargetFail,
    /// Rollback succeeded
    RollbackOk,
    /// Rollback failed (retry)
    RollbackFail,
}

/// State transition function
///
/// Given the current state and an event, returns the next state.
/// Invalid transitions return the current state (no change).
pub fn transition(current: TransferState, event: TransferEvent) -> TransferState {
    use TransferEvent::*;
    use TransferState::*;

    match (current, event) {
        // From Init
        (Init, SourceCall) => SourcePending,
        (Init, SourceOk) => SourceDone,
        (Init, SourceFail) => Failed,

        // From SourcePending
        (SourcePending, SourceOk) => SourceDone,
        (SourcePending, SourceFail) => Failed,

        // From SourceDone
        (SourceDone, TargetCall) => TargetPending,
        (SourceDone, TargetOk) => Committed,
        (SourceDone, TargetFail) => Compensating,

        // From TargetPending
        (TargetPending, TargetOk) => Committed,
        (TargetPending, TargetFail) => Compensating,

        // From Compensating
        (Compensating, RollbackOk) => RolledBack,
        (Compensating, RollbackFail) => Compensating, // Retry

        // Invalid transitions - stay in current state
        _ => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== State Property Tests =====

    #[test]
    fn test_terminal_states() {
        assert!(TransferState::Committed.is_terminal());
        assert!(TransferState::RolledBack.is_terminal());
        assert!(TransferState::Failed.is_terminal());

        assert!(!TransferState::Init.is_terminal());
        assert!(!TransferState::SourcePending.is_terminal());
        assert!(!TransferState::SourceDone.is_terminal());
        assert!(!TransferState::TargetPending.is_terminal());
        assert!(!TransferState::Compensating.is_terminal());
    }

    #[test]
    fn test_needs_processing() {
        assert!(!TransferState::Committed.needs_processing());
        assert!(!TransferState::RolledBack.needs_processing());
        assert!(!TransferState::Failed.needs_processing());

        assert!(TransferState::Init.needs_processing());
        assert!(TransferState::SourcePending.needs_processing());
        assert!(TransferState::SourceDone.needs_processing());
        assert!(TransferState::TargetPending.needs_processing());
        assert!(TransferState::Compensating.needs_processing());
    }

    // ===== State Serialization Tests =====

    #[test]
    fn test_state_to_string_roundtrip() {
        let states = vec![
            TransferState::Init,
            TransferState::SourcePending,
            TransferState::SourceDone,
            TransferState::TargetPending,
            TransferState::Compensating,
            TransferState::Committed,
            TransferState::RolledBack,
            TransferState::Failed,
        ];

        for state in states {
            let s = state.as_str();
            let parsed = TransferState::from_str(s).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn test_invalid_state_string() {
        assert!(TransferState::from_str("invalid").is_none());
        assert!(TransferState::from_str("").is_none());
        assert!(TransferState::from_str("COMMITTED").is_none());
    }

    // ===== Happy Path Transitions =====

    #[test]
    fn test_happy_path_immediate_success() {
        let mut state = TransferState::Init;

        state = transition(state, TransferEvent::SourceOk);
        assert_eq!(state, TransferState::SourceDone);

        state = transition(state, TransferEvent::TargetOk);
        assert_eq!(state, TransferState::Committed);
    }

    #[test]
    fn test_happy_path_with_pending() {
        let mut state = TransferState::Init;

        state = transition(state, TransferEvent::SourceCall);
        assert_eq!(state, TransferState::SourcePending);

        state = transition(state, TransferEvent::SourceOk);
        assert_eq!(state, TransferState::SourceDone);

        state = transition(state, TransferEvent::TargetCall);
        assert_eq!(state, TransferState::TargetPending);

        state = transition(state, TransferEvent::TargetOk);
        assert_eq!(state, TransferState::Committed);
    }

    // ===== Failure Path Transitions =====

    #[test]
    fn test_source_failure_from_init() {
        let state = transition(TransferState::Init, TransferEvent::SourceFail);
        assert_eq!(state, TransferState::Failed);
    }

    #[test]
    fn test_source_failure_from_pending() {
        let state = transition(TransferState::SourcePending, TransferEvent::SourceFail);
        assert_eq!(state, TransferState::Failed);
    }

    #[test]
    fn test_target_failure_triggers_compensation() {
        let state = transition(TransferState::SourceDone, TransferEvent::TargetFail);
        assert_eq!(state, TransferState::Compensating);
    }

    #[test]
    fn test_target_failure_from_pending() {
        let state = transition(TransferState::TargetPending, TransferEvent::TargetFail);
        assert_eq!(state, TransferState::Compensating);
    }

    // ===== Compensation Path =====

    #[test]
    fn test_rollback_success() {
        let state = transition(TransferState::Compensating, TransferEvent::RollbackOk);
        assert_eq!(state, TransferState::RolledBack);
    }

    #[test]
    fn test_rollback_failure_stays_compensating() {
        let state = transition(TransferState::Compensating, TransferEvent::RollbackFail);
        assert_eq!(state, TransferState::Compensating);
    }

    // ===== Invalid Transitions =====

    #[test]
    fn test_terminal_state_is_stable() {
        let state = transition(TransferState::Committed, TransferEvent::SourceFail);
        assert_eq!(state, TransferState::Committed);

        let state = transition(TransferState::RolledBack, TransferEvent::TargetOk);
        assert_eq!(state, TransferState::RolledBack);

        let state = transition(TransferState::Failed, TransferEvent::RollbackOk);
        assert_eq!(state, TransferState::Failed);
    }

    #[test]
    fn test_invalid_transition_stays_in_current() {
        let state = transition(TransferState::Init, TransferEvent::TargetCall);
        assert_eq!(state, TransferState::Init);

        let state = transition(TransferState::Init, TransferEvent::RollbackOk);
        assert_eq!(state, TransferState::Init);
    }
}
