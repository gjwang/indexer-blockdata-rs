use serde::{Deserialize, Serialize};
use std::fmt;

/// States of the Internal Transfer lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InternalTransferState {
    Requesting,
    #[serde(rename = "processing_ubs")]
    Processing,
    Pending,
    Success,
    Failed,
}

impl InternalTransferState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requesting => "requesting",
            Self::Processing => "processing_ubs",
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for InternalTransferState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Events driving the FSM logic
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InternalTransferEvent {
    Submit,
    RouteToUbs,
    LockFunds,
    Settle,
    Fail,
}

/// The Finite State Machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalTransferStateMachine {
    pub state: InternalTransferState,
}

impl InternalTransferStateMachine {
    /// Create new FSM in initial state
    pub fn new() -> Self {
        Self { state: InternalTransferState::Requesting }
    }

    /// Access current state
    pub fn state(&self) -> InternalTransferState {
        self.state
    }

    /// Consumes an event and transitions the state.
    /// Returns Ok previous_state on success, Err if invalid transition.
    pub fn consume(&mut self, event: InternalTransferEvent) -> Result<InternalTransferState, String> {
        let prev_state = self.state;
        let new_state = match (prev_state, event) {
            // Transitions from Requesting
            (InternalTransferState::Requesting, InternalTransferEvent::Submit) => InternalTransferState::Processing,
            (InternalTransferState::Requesting, InternalTransferEvent::Fail) => InternalTransferState::Failed,

            // Transitions from Processing
            (InternalTransferState::Processing, InternalTransferEvent::RouteToUbs) => InternalTransferState::Processing, // Self-transition
            (InternalTransferState::Processing, InternalTransferEvent::LockFunds) => InternalTransferState::Pending,
            (InternalTransferState::Processing, InternalTransferEvent::Fail) => InternalTransferState::Failed,

            // Transitions from Pending
            (InternalTransferState::Pending, InternalTransferEvent::Settle) => InternalTransferState::Success,
            (InternalTransferState::Pending, InternalTransferEvent::Fail) => InternalTransferState::Failed,

            // Terminal states
            (InternalTransferState::Success, _) => return Err(format!("Cannot transition from terminal state Success with event {:?}", event)),
            (InternalTransferState::Failed, _) => return Err(format!("Cannot transition from terminal state Failed with event {:?}", event)),

            // Invalid
            _ => return Err(format!("Invalid transition from {:?} with event {:?}", prev_state, event)),
        };

        self.state = new_state;
        Ok(prev_state)
    }

    /// Check if terminal
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, InternalTransferState::Success | InternalTransferState::Failed)
    }

    /// Helper compatibility (similar to mapping)
    pub fn as_str(&self) -> &'static str {
        self.state.as_str()
    }
}

impl Default for InternalTransferStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InternalTransferStateMachine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.state)
    }
}
