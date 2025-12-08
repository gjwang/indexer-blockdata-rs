//! Simple logging macros
//!
//! Usage in any service - just 2 lines:
//! ```rust,ignore
//! const TARGET: &str = "UBSC";
//! fetcher::define_log_macros!(TARGET);
//!
//! // Then use:
//! info!("Starting service");
//! error!("Failed: {}", err);
//! ```

/// Define logging macros (info!, warn!, error!, debug!) with a fixed target.
///
/// Usage:
/// ```rust,ignore
/// const TARGET: &str = "MyService";
/// fetcher::define_log_macros!(TARGET);
/// ```
#[macro_export]
macro_rules! define_log_macros {
    ($target:expr) => {
        macro_rules! info  { ($($arg:tt)*) => { log::info!(target: $target, $($arg)*) } }
        macro_rules! warn  { ($($arg:tt)*) => { log::warn!(target: $target, $($arg)*) } }
        macro_rules! error { ($($arg:tt)*) => { log::error!(target: $target, $($arg)*) } }
        macro_rules! debug { ($($arg:tt)*) => { log::debug!(target: $target, $($arg)*) } }
    };
}
