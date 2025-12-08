//! Aeron configuration for UBSCore communication
//!
//! Defines channels and stream IDs for UDP communication.
//! One port = one direction in Aeron UDP.

/// Aeron IPC channel (shared memory - same machine only)
pub const IPC_CHANNEL: &str = "aeron:ipc";

/// Default UDP ports
pub mod ports {
    /// Gateway → UBSCore: Order requests
    pub const ORDERS: u16 = 40456;
    /// UBSCore → Gateway: Order responses
    pub const RESPONSES: u16 = 40457;
    /// UBSCore → ME: Validated orders
    pub const ORDERS_TO_ME: u16 = 40458;
    /// ME → UBSCore: Trade fills
    pub const FILLS: u16 = 40459;
}

/// Stream IDs for different message types
pub mod streams {
    /// Gateway → UBSCore: Order requests
    pub const ORDERS_IN: i32 = 1001;

    /// UBSCore → Matching Engine: Validated orders
    pub const ORDERS_OUT: i32 = 1002;

    /// Matching Engine → UBSCore: Trade fills
    pub const FILLS_IN: i32 = 1003;

    /// UBSCore → Gateway: Order responses (accept/reject)
    pub const RESPONSES_OUT: i32 = 1004;
}

/// Configuration for Aeron communication
#[derive(Debug, Clone)]
pub struct AeronConfig {
    /// Channel for receiving orders (Gateway → UBSCore)
    pub orders_channel: String,

    /// Channel for sending responses (UBSCore → Gateway)
    pub responses_channel: String,

    /// Stream ID for incoming orders
    pub orders_in_stream: i32,

    /// Stream ID for outgoing validated orders
    pub orders_out_stream: i32,

    /// Stream ID for incoming fills
    pub fills_in_stream: i32,

    /// Stream ID for outgoing responses
    pub responses_out_stream: i32,

    /// Idle strategy: busy-spin (lowest latency) or yield
    pub busy_spin: bool,
}

/// Default Aeron UDP endpoints
pub const UDP_ORDERS_ENDPOINT: &str = "localhost:40456";
pub const UDP_RESPONSES_ENDPOINT: &str = "localhost:40457";

impl Default for AeronConfig {
    fn default() -> Self {
        // Use IPC for development (shared memory, same machine)
        Self {
            orders_channel: IPC_CHANNEL.to_string(),
            responses_channel: IPC_CHANNEL.to_string(),
            orders_in_stream: streams::ORDERS_IN,
            orders_out_stream: streams::ORDERS_OUT,
            fills_in_stream: streams::FILLS_IN,
            responses_out_stream: streams::RESPONSES_OUT,
            busy_spin: true,
        }
    }
}

impl AeronConfig {
    /// Create config for UDP unicast (point-to-point)
    /// orders_host: where UBSCore listens for orders
    /// responses_host: where Gateway listens for responses
    pub fn udp(orders_host: &str, responses_host: &str) -> Self {
        Self {
            orders_channel: format!("aeron:udp?endpoint={}", orders_host),
            responses_channel: format!("aeron:udp?endpoint={}", responses_host),
            orders_in_stream: streams::ORDERS_IN,
            orders_out_stream: streams::ORDERS_OUT,
            fills_in_stream: streams::FILLS_IN,
            responses_out_stream: streams::RESPONSES_OUT,
            busy_spin: true,
        }
    }

    /// Create config for IPC (shared memory - same machine only, dev/test)
    pub fn ipc() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_udp() {
        let config = AeronConfig::default();
        assert!(config.orders_channel.contains("aeron:udp"));
        assert!(config.orders_channel.contains("40456"));
        assert!(config.responses_channel.contains("40457"));
        assert_eq!(config.orders_in_stream, 1001);
        assert!(config.busy_spin);
    }

    #[test]
    fn test_ipc_config() {
        let config = AeronConfig::ipc();
        assert_eq!(config.orders_channel, "aeron:ipc");
        assert_eq!(config.responses_channel, "aeron:ipc");
    }

    #[test]
    fn test_udp_unicast() {
        let config = AeronConfig::udp("192.168.1.100:40456", "192.168.1.200:40457");
        assert!(config.orders_channel.contains("192.168.1.100"));
        assert!(config.responses_channel.contains("192.168.1.200"));
    }
}
