//! UBSCore Handler - PURE Business Logic
//!
//! This module contains the order processing business logic,
//! completely separated from transport (Aeron) and metrics.
//!
//! The handler receives raw bytes and returns response bytes.
//! NO stats, NO latency tracking - that belongs to the transport layer.

use std::time::Duration;

use rdkafka::producer::{FutureProducer, FutureRecord};

use crate::ubs_core::{
    MmapWal, RejectReason, SpotRiskModel, UBSCore,
    WalEntry, WalEntryType,
};
use crate::ubs_core::comm::{
    OrderMessage, DepositMessage, WithdrawMessage, ResponseMessage, reason_codes, WireMessage,
};
use super::message::{MsgType, parse_message, build_message};

/// UBSCore message handler - PURE business logic
///
/// Pattern similar to WebSocket/gRPC handlers:
/// ```
/// fn on_message(payload: &[u8]) -> Vec<u8> {
///     let (msg_type, body) = parse_message(payload)?;
///     match msg_type {
///         MsgType::Order => handle_order(body),
///         MsgType::Cancel => handle_cancel(body),
///         _ => error_response(),
///     }
/// }
/// ```
pub struct UbsCoreHandler<'a> {
    pub ubs_core: &'a mut UBSCore<SpotRiskModel>,
    pub wal: &'a mut MmapWal,
    pub kafka_producer: &'a Option<FutureProducer>,
}

impl<'a> UbsCoreHandler<'a> {
    /// Process raw message bytes and return response bytes
    ///
    /// Wire format: [1-byte msg_type] + [message_body]
    pub fn on_message(&mut self, payload: &[u8]) -> Vec<u8> {
        // Parse message type
        let (msg_type, body) = match parse_message(payload) {
            Some(r) => r,
            None => {
                log::warn!("[UBSC] Empty message");
                return self.error_response(0, reason_codes::INTERNAL_ERROR);
            }
        };

        // Dispatch by message type
        match msg_type {
            MsgType::Order => self.handle_order(body),
            MsgType::Cancel => self.handle_cancel(body),
            MsgType::Query => self.handle_query(body),
            MsgType::Deposit => self.handle_deposit(body),
            MsgType::Withdraw => self.handle_withdraw(body),
            MsgType::Response => {
                log::warn!("[UBSC] Unexpected response message");
                self.error_response(0, reason_codes::INTERNAL_ERROR)
            }
        }
    }

    /// Handle ORDER message
    fn handle_order(&mut self, body: &[u8]) -> Vec<u8> {
        // Parse order from body
        let order_msg = match OrderMessage::from_bytes(body) {
            Some(msg) => msg,
            None => {
                log::warn!("[UBSC] Invalid order message");
                return self.error_response(0, reason_codes::INTERNAL_ERROR);
            }
        };

        // Convert to InternalOrder
        let order = match order_msg.to_internal_order() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("[UBSC] Order conversion failed: {:?}", e);
                return self.reject_response(order_msg.order_id, reason_codes::INVALID_SYMBOL);
            }
        };

        // 1. VALIDATE
        if let Err(reason) = self.ubs_core.validate_order(&order) {
            let reason_code = match reason {
                RejectReason::InsufficientBalance => reason_codes::INSUFFICIENT_BALANCE,
                RejectReason::DuplicateOrderId => reason_codes::DUPLICATE_ORDER_ID,
                RejectReason::AccountNotFound => reason_codes::ACCOUNT_NOT_FOUND,
                _ => reason_codes::INTERNAL_ERROR,
            };
            return self.reject_response(order.order_id, reason_code);
        }

        // 2. WAL APPEND
        if let Ok(payload) = bincode::serialize(&order) {
            let entry = WalEntry::new(WalEntryType::OrderLock, payload);
            if let Err(e) = self.wal.append(&entry) {
                log::error!("[UBSC] WAL append failed: {:?}", e);
                return self.reject_response(order.order_id, reason_codes::INTERNAL_ERROR);
            }
        }

        // 3. WAL FLUSH
        if let Err(e) = self.wal.flush() {
            log::error!("[UBSC] WAL flush failed: {:?}", e);
        }

        // 4. Forward to Kafka (async, best-effort)
        if let Some(producer) = self.kafka_producer {
            let payload = bincode::serialize(&order).unwrap_or_default();
            let key = order.order_id.to_string();
            let record = FutureRecord::to("validated_orders")
                .payload(&payload)
                .key(&key);
            let _ = producer.send(record, Duration::from_secs(0));
        }

        self.accept_response(order.order_id)
    }

    /// Handle CANCEL message (stub for future)
    fn handle_cancel(&mut self, _body: &[u8]) -> Vec<u8> {
        log::warn!("[UBSC] Cancel not implemented yet");
        self.error_response(0, reason_codes::INTERNAL_ERROR)
    }

    /// Handle QUERY message (stub for future)
    fn handle_query(&mut self, _body: &[u8]) -> Vec<u8> {
        log::warn!("[UBSC] Query not implemented yet");
        self.error_response(0, reason_codes::INTERNAL_ERROR)
    }

    /// Handle DEPOSIT message
    fn handle_deposit(&mut self, body: &[u8]) -> Vec<u8> {
        let msg = match DepositMessage::from_bytes(body) {
            Some(m) => m,
            None => {
                log::warn!("[UBSC] Invalid deposit message");
                return self.error_response(0, reason_codes::INTERNAL_ERROR);
            }
        };

        self.ubs_core.on_deposit(msg.user_id, msg.asset_id, msg.amount);
        log::info!("[UBSC] Deposit: user={} asset={} amount={}", msg.user_id, msg.asset_id, msg.amount);
        self.accept_response(msg.user_id)
    }

    /// Handle WITHDRAW message
    fn handle_withdraw(&mut self, body: &[u8]) -> Vec<u8> {
        let msg = match WithdrawMessage::from_bytes(body) {
            Some(m) => m,
            None => {
                log::warn!("[UBSC] Invalid withdraw message");
                return self.error_response(0, reason_codes::INTERNAL_ERROR);
            }
        };

        if self.ubs_core.on_withdraw(msg.user_id, msg.asset_id, msg.amount) {
            log::info!("[UBSC] Withdraw: user={} asset={} amount={}", msg.user_id, msg.asset_id, msg.amount);
            self.accept_response(msg.user_id)
        } else {
            log::warn!("[UBSC] Withdraw failed: insufficient balance");
            self.reject_response(msg.user_id, reason_codes::INSUFFICIENT_BALANCE)
        }
    }

    // --- Response builders ---

    fn accept_response(&self, order_id: u64) -> Vec<u8> {
        build_message(MsgType::Response, &ResponseMessage::accept(order_id))
    }

    fn reject_response(&self, order_id: u64, reason_code: u8) -> Vec<u8> {
        build_message(MsgType::Response, &ResponseMessage::reject(order_id, reason_code))
    }

    fn error_response(&self, order_id: u64, reason_code: u8) -> Vec<u8> {
        build_message(MsgType::Response, &ResponseMessage::reject(order_id, reason_code))
    }
}
