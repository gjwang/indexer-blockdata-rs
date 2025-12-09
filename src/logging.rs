/// Phase 3 Logging - Structured async logging utilities
///
/// Provides helpers for:
/// - Structured JSON logging
/// - Event tracing across services
/// - Metrics integration
/// - Helper macros for common patterns

use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// Get current timestamp in milliseconds
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Get hostname for log identification
pub fn hostname() -> String {
    hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Structured log event builder
///
/// Usage:
/// ```
/// use fetcher::logging::LogEvent;
///
/// let log_value = LogEvent::new("DEPOSIT_CONSUMED")
///     .field("event_id", "deposit_1001_1_123456")
///     .field("user_id", 1001)
///     .field("amount", 1000000)
///     .service("ubscore")
///     .build();
///
/// tracing::info!("{}", log_value);
/// ```
pub struct LogEvent {
    fields: serde_json::Map<String, Value>,
}

impl LogEvent {
    /// Create a new log event with the given event name
    pub fn new(event: &str) -> Self {
        let mut fields = serde_json::Map::new();
        fields.insert("event".to_string(), json!(event));
        fields.insert("timestamp_ms".to_string(), json!(now_ms()));

        Self { fields }
    }

    /// Add a field to the log event
    pub fn field(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.fields.insert(key.to_string(), value.into());
        self
    }

    /// Add service name
    pub fn service(mut self, service: &str) -> Self {
        self.fields.insert("service".to_string(), json!(service));
        self
    }

    /// Add host information
    pub fn with_host(mut self) -> Self {
        self.fields.insert("host".to_string(), json!(hostname()));
        self
    }

    /// Build the final JSON value
    pub fn build(self) -> Value {
        Value::Object(self.fields)
    }
}

/// Helper macros for common log events
#[macro_export]
macro_rules! log_deposit_consumed {
    ($event_id:expr, $user_id:expr, $asset_id:expr, $amount:expr) => {
        tracing::info!(
            "{}",
            $crate::logging::LogEvent::new("DEPOSIT_CONSUMED")
                .field("event_id", $event_id)
                .field("user_id", $user_id)
                .field("asset_id", $asset_id)
                .field("amount", $amount)
                .service("ubscore")
                .build()
        );
    };
}

#[macro_export]
macro_rules! log_deposit_validated {
    ($event_id:expr, $user_id:expr, $balance_before:expr, $balance_after:expr) => {
        tracing::info!(
            "{}",
            $crate::logging::LogEvent::new("DEPOSIT_VALIDATED")
                .field("event_id", $event_id)
                .field("user_id", $user_id)
                .field("balance_before", $balance_before)
                .field("balance_after", $balance_after)
                .field("delta", $balance_after - $balance_before)
                .service("ubscore")
                .build()
        );
    };
}

/// Generate trace_id for batch operations
pub fn gen_batch_trace_id(seq: u64) -> String {
    format!("batch_{}_{}", seq, now_ms())
}

/// Generate trace_id for workflows
pub fn gen_flow_trace_id(flow_type: &str, unique: &str) -> String {
    format!("flow_{}_{}_{}", flow_type, unique, now_ms())
}

/// Generate trace_id for distributed requests
pub fn gen_request_trace_id(seq: u64) -> String {
    format!("req_{}_{}", seq, now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_event_builder() {
        let log = LogEvent::new("TEST_EVENT")
            .field("user_id", 1001)
            .field("amount", 1000000)
            .service("test")
            .build();

        assert_eq!(log["event"], "TEST_EVENT");
        assert_eq!(log["user_id"], 1001);
        assert_eq!(log["amount"], 1000000);
        assert_eq!(log["service"], "test");
        assert!(log.get("timestamp_ms").is_some());
    }

    #[test]
    fn test_trace_id_format() {
        let batch_id = gen_batch_trace_id(12345);
        assert!(batch_id.starts_with("batch_12345_"));

        let flow_id = gen_flow_trace_id("reg", "abc123");
        assert!(flow_id.starts_with("flow_reg_abc123_"));

        let req_id = gen_request_trace_id(98765);
        assert!(req_id.starts_with("req_98765_"));
    }
}
