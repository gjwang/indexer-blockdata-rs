//! UDP Server for UBSCore Service
//!
//! Receives orders from Gateway via UDP, processes them, and sends responses.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::ubs_client::{OrderRequest, ResponseMessage};
use crate::ubs_core::order::{InternalOrder, OrderType, Side};
use crate::ubs_core::RejectReason;

/// UDP Server for UBSCore
pub struct UbsServer {
    socket: UdpSocket,
    running: Arc<AtomicBool>,
}

impl UbsServer {
    /// Create and bind server
    pub fn new(port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port))?;
        socket.set_nonblocking(false)?;

        Ok(Self {
            socket,
            running: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Get running flag for shutdown
    pub fn running(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Stop the server
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Receive next order request (blocking)
    pub fn recv_order(&self) -> Option<(OrderRequest, std::net::SocketAddr)> {
        let mut buf = [0u8; 256];
        match self.socket.recv_from(&mut buf) {
            Ok((len, addr)) => {
                if len >= std::mem::size_of::<OrderRequest>() {
                    let req = unsafe {
                        let ptr = buf.as_ptr() as *const OrderRequest;
                        ptr.read_unaligned()
                    };
                    Some((req, addr))
                } else {
                    log::warn!("[UBS_UDP] Received undersized packet: {} bytes", len);
                    None
                }
            }
            Err(e) => {
                if e.kind() != std::io::ErrorKind::WouldBlock {
                    log::error!("[UBS_UDP] Recv error: {}", e);
                }
                None
            }
        }
    }

    /// Send response back to Gateway
    pub fn send_response(&self, addr: std::net::SocketAddr, response: ResponseMessage) -> std::io::Result<()> {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &response as *const ResponseMessage as *const u8,
                std::mem::size_of::<ResponseMessage>(),
            )
        };
        self.socket.send_to(bytes, addr)?;
        Ok(())
    }

    /// Convert OrderRequest to InternalOrder
    pub fn to_internal_order(req: &OrderRequest) -> Result<InternalOrder, RejectReason> {
        let side = match req.side {
            0 => Side::Buy,
            1 => Side::Sell,
            _ => return Err(RejectReason::InvalidSymbol),
        };

        let order_type = match req.order_type {
            0 => OrderType::Limit,
            1 => OrderType::Market,
            _ => return Err(RejectReason::InvalidSymbol),
        };

        Ok(InternalOrder {
            order_id: req.order_id,
            user_id: req.user_id,
            symbol_id: req.symbol_id,
            side,
            order_type,
            price: req.price,
            qty: req.qty,
        })
    }
}

/// Create accept response
pub fn accept_response(request_id: u64, order_id: u64) -> ResponseMessage {
    ResponseMessage {
        request_id,
        order_id,
        accepted: 1,
        reason_code: 0,
    }
}

/// Create reject response
pub fn reject_response(request_id: u64, order_id: u64, reason: &RejectReason) -> ResponseMessage {
    let reason_code = match reason {
        RejectReason::InsufficientBalance => 1,
        RejectReason::DuplicateOrderId => 2,
        RejectReason::OrderTooOld => 3,
        RejectReason::FutureTimestamp => 4,
        RejectReason::InvalidSymbol => 5,
        RejectReason::OrderCostOverflow => 6,
        RejectReason::AccountNotFound => 7,
        RejectReason::SystemBusy => 8,
        RejectReason::InternalError => 99,
    };
    ResponseMessage {
        request_id,
        order_id,
        accepted: 0,
        reason_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_creation() {
        let resp = accept_response(123, 456);
        assert_eq!(resp.request_id, 123);
        assert_eq!(resp.order_id, 456);
        assert_eq!(resp.accepted, 1);
    }
}
