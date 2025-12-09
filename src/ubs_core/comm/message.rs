//! Generic Message Protocol
//!
//! Wire format: [1-byte msg_type] + [payload]
//!
//! This allows the transport layer to be completely generic.
//! Business logic defines message types and handlers.

use super::WireMessage;

/// Message types
pub mod msg_type {
    pub const ORDER: u8 = 1;
    pub const CANCEL: u8 = 2;
    pub const QUERY: u8 = 3;
    pub const RESPONSE: u8 = 128;  // Response flag
}

/// Parse message type from raw bytes
/// Returns (msg_type, body) or None if empty
pub fn parse_message(payload: &[u8]) -> Option<(u8, &[u8])> {
    if payload.is_empty() {
        return None;
    }
    Some((payload[0], &payload[1..]))
}

/// Build message with type prefix
pub fn build_message<T: WireMessage>(msg_type: u8, msg: &T) -> Vec<u8> {
    let body = msg.to_bytes();
    let mut result = Vec::with_capacity(1 + body.len());
    result.push(msg_type);
    result.extend_from_slice(body);
    result
}

/// Build message with type prefix (from bytes)
pub fn build_message_bytes(msg_type: u8, body: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(1 + body.len());
    result.push(msg_type);
    result.extend_from_slice(body);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message() {
        let raw = [msg_type::ORDER, 1, 2, 3, 4];
        let (mt, body) = parse_message(&raw).unwrap();
        assert_eq!(mt, msg_type::ORDER);
        assert_eq!(body, &[1, 2, 3, 4]);
    }
}
