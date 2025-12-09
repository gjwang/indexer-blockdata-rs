/// Async logging setup utilities
///
/// Provides easy setup for async file logging with rotation

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Setup async file logging with daily rotation
///
/// Returns WorkerGuard that MUST be kept alive for the entire program
///
/// # Example
/// ```
/// fn main() {
///     let _guard = setup_async_file_logging("ubscore", "logs");
///     // ... rest of program
/// }
/// ```
pub fn setup_async_file_logging(service_name: &str, log_dir: &str) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(log_dir, format!("{}.log", service_name));
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .json() // Structured JSON output
                .with_target(true)
                .with_thread_ids(true)
                .with_file(false)
                .with_line_number(false)
        )
        .init();

    guard
}

/// Setup async logging with both file and stdout
///
/// Useful for development - logs go to both file and console
pub fn setup_dual_logging(service_name: &str, log_dir: &str) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(log_dir, format!("{}.log", service_name));
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .json()
                .with_target(true)
                .with_thread_ids(true)
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stdout)
                .with_target(false) // Cleaner console output
        )
        .init();

    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_setup() {
        // Just verify it doesn't panic
        let _guard = setup_async_file_logging("test", "/tmp");
        tracing::info!("test log");
    }
}
