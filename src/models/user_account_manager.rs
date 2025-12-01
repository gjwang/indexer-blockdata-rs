use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct UserAccountManager {
    // Simulating a database or internal state
    next_user_id: AtomicU64,
}

impl UserAccountManager {
    pub fn new() -> Self {
        Self {
            next_user_id: AtomicU64::new(1001),
        }
    }

    /// Simulate getting a user ID.
    /// In a real app, this would likely take an auth token or session ID.
    /// For now, we'll just return a simulated user ID.
    pub fn get_user_id(&self) -> u64 {
        // For simulation, let's just return a fixed user or rotate them.
        // If we want to simulate multiple users, we could increment.
        // But typically a request comes from ONE user.
        // Let's just return a fixed ID for the "demo" user.
        1001
    }
}

impl Default for UserAccountManager {
    fn default() -> Self {
        Self::new()
    }
}
