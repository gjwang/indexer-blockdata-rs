//! Generic Message Protocol
//!
//! Wire format: [1-byte msg_type] + [payload]
//!
//! This allows the transport layer to be completely generic.
//! Business logic defines message types and handlers.

use super::WireMessage;

/// Message type enum with explicit wire values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    Order = 1,
    Cancel = 2,
    Query = 3,
    Response = 128,
}

impl TryFrom<u8> for MsgType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(MsgType::Order),
            2 => Ok(MsgType::Cancel),
            3 => Ok(MsgType::Query),
            128 => Ok(MsgType::Response),
            _ => Err(()),
        }
    }
}

impl From<MsgType> for u8 {
    fn from(mt: MsgType) -> u8 {
        mt as u8
    }
}

/// Parse message type from raw bytes
/// Returns (MsgType, body) or None if invalid
pub fn parse_message(payload: &[u8]) -> Option<(MsgType, &[u8])> {
    if payload.is_empty() {
        return None;
    }
    let msg_type = MsgType::try_from(payload[0]).ok()?;
    Some((msg_type, &payload[1..]))
}

/// Build message with type prefix
pub fn build_message<T: WireMessage>(msg_type: MsgType, msg: &T) -> Vec<u8> {
    let body = msg.to_bytes();
    let mut result = Vec::with_capacity(1 + body.len());
    result.push(msg_type as u8);
    result.extend_from_slice(body);
    result
}

/// Build message with type prefix (from bytes)
pub fn build_message_bytes(msg_type: MsgType, body: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(1 + body.len());
    result.push(msg_type as u8);
    result.extend_from_slice(body);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message() {
        let raw = [MsgType::Order as u8, 1, 2, 3, 4];
        let (mt, body) = parse_message(&raw).unwrap();
        assert_eq!(mt, MsgType::Order);
        assert_eq!(body, &[1, 2, 3, 4]);
    }

    #[test]
    fn test_msg_type_roundtrip() {
        assert_eq!(MsgType::try_from(1).unwrap(), MsgType::Order);
        assert_eq!(MsgType::try_from(128).unwrap(), MsgType::Response);
        assert!(MsgType::try_from(255).is_err());
    }
}
