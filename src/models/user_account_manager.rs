use rand::Rng;

#[derive(Debug)]
pub struct UserAccountManager;

impl UserAccountManager {
    pub fn new() -> Self {
        Self
    }

    /// Simulate getting a user ID.
    /// In a real app, this would likely take an auth token or session ID.
    /// For now, we'll just return a simulated random user ID.
    pub fn get_user_id(&self) -> u64 {
        // Generate a random user ID between 1000 and 2000
        let mut rng = rand::rng();
        rng.random_range(1000..2000)
    }
}

impl Default for UserAccountManager {
    fn default() -> Self {
        Self::new()
    }
}
