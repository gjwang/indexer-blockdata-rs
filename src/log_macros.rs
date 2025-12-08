//! Simple targeted logging
//!
//! Usage:
//! ```rust,ignore
//! use fetcher::log_macros::Logger;
//!
//! static L: Logger = Logger::new("UBSC");
//!
//! // Simple messages - clean!
//! L.info("Starting service");
//! L.error("Connection failed");
//!
//! // With variables - use &format!()
//! L.info(&format!("Port: {}", 8080));
//! L.error(&format!("Error: {}", err));
//! ```

/// A simple logger with a fixed target.
#[derive(Debug, Clone, Copy)]
pub struct Logger {
    pub target: &'static str,
}

impl Logger {
    pub const fn new(target: &'static str) -> Self {
        Self { target }
    }

    #[inline]
    pub fn info(&self, msg: &str) {
        log::info!(target: self.target, "{}", msg);
    }

    #[inline]
    pub fn warn(&self, msg: &str) {
        log::warn!(target: self.target, "{}", msg);
    }

    #[inline]
    pub fn error(&self, msg: &str) {
        log::error!(target: self.target, "{}", msg);
    }

    #[inline]
    pub fn debug(&self, msg: &str) {
        log::debug!(target: self.target, "{}", msg);
    }
}
