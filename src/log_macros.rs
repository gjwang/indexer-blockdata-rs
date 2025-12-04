/// Logging macros with custom target for cleaner log output
/// 
/// Instead of showing the full binary name (e.g., "settlement_service"),
/// these macros use a shorter, cleaner target name (e.g., "settlement").

#[macro_export]
macro_rules! log_info {
    ($target:expr, $($arg:tt)*) => {
        log::info!(target: $target, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_error {
    ($target:expr, $($arg:tt)*) => {
        log::error!(target: $target, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_warn {
    ($target:expr, $($arg:tt)*) => {
        log::warn!(target: $target, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_debug {
    ($target:expr, $($arg:tt)*) => {
        log::debug!(target: $target, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_trace {
    ($target:expr, $($arg:tt)*) => {
        log::trace!(target: $target, $($arg)*)
    };
}
