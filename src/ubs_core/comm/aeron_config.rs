//! Aeron configuration for UBSCore communication
//!
//! Defines channels and stream IDs for IPC communication.

/// Aeron IPC channel (shared memory)
pub const IPC_CHANNEL: &str = "aeron:ipc";

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
    /// Channel for orders (default: aeron:ipc)
    pub channel: String,

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

/// Default Aeron UDP endpoint for UBSCore
pub const UDP_ENDPOINT: &str = "localhost:40456";

impl Default for AeronConfig {
    fn default() -> Self {
        // Default to UDP transport (works across servers in production)
        Self {
            channel: format!("aeron:udp?endpoint={}", UDP_ENDPOINT),
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
    pub fn udp(endpoint: &str) -> Self {
        Self { channel: format!("aeron:udp?endpoint={}", endpoint), ..Default::default() }
    }

    /// Create config for UDP multicast (for distributed deployment)
    pub fn udp_multicast(address: &str) -> Self {
        Self { channel: format!("aeron:udp?endpoint={}", address), ..Default::default() }
    }

    /// Create config for IPC (shared memory - same machine only, dev/test)
    pub fn ipc() -> Self {
        Self { channel: IPC_CHANNEL.to_string(), ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_udp() {
        let config = AeronConfig::default();
        assert!(config.channel.contains("aeron:udp"));
        assert!(config.channel.contains(UDP_ENDPOINT));
        assert_eq!(config.orders_in_stream, 1001);
        assert!(config.busy_spin);
    }

    #[test]
    fn test_ipc_config() {
        let config = AeronConfig::ipc();
        assert_eq!(config.channel, "aeron:ipc");
    }

    #[test]
    fn test_udp_unicast() {
        let config = AeronConfig::udp("192.168.1.100:40456");
        assert!(config.channel.contains("192.168.1.100"));
    }
}
