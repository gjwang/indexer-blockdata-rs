//! Aeron UDP Client for Gateway
//!
//! Sends orders to UBSCore via Aeron UDP and receives responses.

use std::net::UdpSocket;
use std::time::Duration;

use crate::ubs_core::order::InternalOrder;

/// Response from UBSCore
#[derive(Debug, Clone)]
pub enum UbsResponse {
    Accepted { order_id: u64 },
    Rejected { order_id: u64, reason: String },
}

/// Wire format for order request (fixed size for speed)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct OrderRequest {
    pub request_id: u64,
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,       // 0 = Buy, 1 = Sell
    pub order_type: u8, // 0 = Limit, 1 = Market
    pub price: u64,
    pub qty: u64,
}

impl OrderRequest {
    pub fn from_order(request_id: u64, order: &InternalOrder) -> Self {
        Self {
            request_id,
            order_id: order.order_id,
            user_id: order.user_id,
            symbol_id: order.symbol_id,
            side: match order.side {
                crate::ubs_core::Side::Buy => 0,
                crate::ubs_core::Side::Sell => 1,
            },
            order_type: match order.order_type {
                crate::ubs_core::OrderType::Limit => 0,
                crate::ubs_core::OrderType::Market => 1,
            },
            price: order.price,
            qty: order.qty,
        }
    }

    pub fn to_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }
}

/// Wire format for response (fixed size)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ResponseMessage {
    pub request_id: u64,
    pub order_id: u64,
    pub accepted: u8,      // 1 = accepted, 0 = rejected
    pub reason_code: u8,   // Rejection reason code
}

impl ResponseMessage {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < std::mem::size_of::<Self>() {
            return None;
        }
        unsafe {
            let ptr = bytes.as_ptr() as *const Self;
            Some(ptr.read_unaligned())
        }
    }
}

/// Aeron UDP client for Gateway
pub struct UbsClient {
    socket: UdpSocket,
    ubs_addr: String,
    timeout: Duration,
    next_request_id: u64,
}

impl UbsClient {
    /// Create new client
    pub fn new(ubs_host: &str, ubs_port: u16) -> std::io::Result<Self> {
        // Bind to any available port for receiving responses
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;

        Ok(Self {
            socket,
            ubs_addr: format!("{}:{}", ubs_host, ubs_port),
            timeout: Duration::from_millis(100),
            next_request_id: 1,
        })
    }

    /// Send order and wait for response
    pub fn send_order(&mut self, order: &InternalOrder) -> Result<UbsResponse, String> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let request = OrderRequest::from_order(request_id, order);

        // Send
        self.socket
            .send_to(request.to_bytes(), &self.ubs_addr)
            .map_err(|e| format!("Send failed: {}", e))?;

        // Wait for response
        let mut buf = [0u8; 64];
        match self.socket.recv_from(&mut buf) {
            Ok((len, _addr)) => {
                if let Some(resp) = ResponseMessage::from_bytes(&buf[..len]) {
                    // Copy packed fields to avoid alignment issues
                    let resp_request_id = resp.request_id;
                    let resp_order_id = resp.order_id;
                    let resp_accepted = resp.accepted;
                    let resp_reason_code = resp.reason_code;

                    if resp_request_id == request_id {
                        if resp_accepted == 1 {
                            Ok(UbsResponse::Accepted { order_id: resp_order_id })
                        } else {
                            Ok(UbsResponse::Rejected {
                                order_id: resp_order_id,
                                reason: format!("Reason code: {}", resp_reason_code),
                            })
                        }
                    } else {
                        Err(format!("Request ID mismatch: expected {}, got {}", request_id, resp_request_id))
                    }
                } else {
                    Err("Invalid response format".to_string())
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Err("Timeout waiting for UBS response".to_string())
            }
            Err(e) => Err(format!("Recv failed: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_request_size() {
        // Should fit in single UDP packet
        assert!(std::mem::size_of::<OrderRequest>() < 1400);
        println!("OrderRequest size: {} bytes", std::mem::size_of::<OrderRequest>());
    }

    #[test]
    fn test_response_message_size() {
        assert!(std::mem::size_of::<ResponseMessage>() < 100);
        println!("ResponseMessage size: {} bytes", std::mem::size_of::<ResponseMessage>());
    }
}
