//! UBSCore Handler - Business Logic Only
//!
//! This module contains the order processing business logic,
//! completely separated from transport (Aeron).
//!
//! The handler receives raw bytes and returns response bytes.
//! It uses a generic message dispatch pattern like WebSocket handlers.

use std::time::Instant;

use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;

use crate::ubs_core::{
    MmapWal, RejectReason, SpotRiskModel, UBSCore,
    WalEntry, WalEntryType,
};
use crate::ubs_core::comm::{
    OrderMessage, ResponseMessage, reason_codes, WireMessage,
};
use super::message::{MsgType, parse_message, build_message};

/// Handler stats
#[derive(Debug, Default)]
pub struct HandlerStats {
    pub received: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub latency_sum_us: u64,
    pub latency_min_us: u64,
    pub latency_max_us: u64,
}

impl HandlerStats {
    pub fn new() -> Self {
        Self {
            latency_min_us: u64::MAX,
            ..Default::default()
        }
    }
}

/// UBSCore message handler - business logic only
///
/// Pattern similar to WebSocket/gRPC handlers:
/// ```
/// fn on_message(payload: &[u8]) -> Vec<u8> {
///     let (msg_type, body) = parse_message(payload)?;
///     match msg_type {
///         MSG_ORDER => handle_order(body),
///         MSG_CANCEL => handle_cancel(body),
///         _ => error_response(),
///     }
/// }
/// ```
pub struct UbsCoreHandler<'a> {
    pub ubs_core: &'a mut UBSCore<SpotRiskModel>,
    pub wal: &'a mut MmapWal,
    pub kafka_producer: &'a Option<FutureProducer>,
    pub stats: &'a mut HandlerStats,
}

impl<'a> UbsCoreHandler<'a> {
    /// Process raw message bytes and return response bytes
    ///
    /// Wire format: [1-byte msg_type] + [message_body]
    ///
    /// This is the message dispatch entry point.
    pub fn on_message(&mut self, payload: &[u8]) -> Vec<u8> {
        self.stats.received += 1;

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
            MsgType::Response => {
                log::warn!("[UBSC] Unexpected response message");
                self.error_response(0, reason_codes::INTERNAL_ERROR)
            }
        }
    }

    /// Handle ORDER message
    fn handle_order(&mut self, body: &[u8]) -> Vec<u8> {
        let start = Instant::now();

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
                self.stats.rejected += 1;
                return self.reject_response(order_msg.order_id, reason_codes::INVALID_SYMBOL);
            }
        };
        let t_parse = start.elapsed();

        // 1. VALIDATE (cheap, no I/O)
        if let Err(reason) = self.ubs_core.validate_order(&order) {
            let reason_code = match reason {
                RejectReason::InsufficientBalance => reason_codes::INSUFFICIENT_BALANCE,
                RejectReason::DuplicateOrderId => reason_codes::DUPLICATE_ORDER_ID,
                RejectReason::AccountNotFound => reason_codes::ACCOUNT_NOT_FOUND,
                _ => reason_codes::INTERNAL_ERROR,
            };
            self.stats.rejected += 1;
            return self.reject_response(order.order_id, reason_code);
        }
        let t_validate = start.elapsed();

        // 2. WAL APPEND
        let t_wal_append_start = Instant::now();
        if let Ok(payload) = bincode::serialize(&order) {
            let entry = WalEntry::new(WalEntryType::OrderLock, payload);
            if let Err(e) = self.wal.append(&entry) {
                log::error!("[UBSC] WAL append failed: {:?}", e);
                self.stats.rejected += 1;
                return self.reject_response(order.order_id, reason_codes::INTERNAL_ERROR);
            }
        }
        let wal_append_us = t_wal_append_start.elapsed().as_micros();

        // 3. WAL FLUSH
        let t_wal_flush_start = Instant::now();
        if let Err(e) = self.wal.flush() {
            log::error!("[UBSC] WAL flush failed: {:?}", e);
        }
        let wal_flush_us = t_wal_flush_start.elapsed().as_micros();

        // 4. Accept!
        self.stats.accepted += 1;

        // 5. Forward to Kafka (async, best-effort)
        if let Some(producer) = self.kafka_producer {
            let payload = bincode::serialize(&order).unwrap_or_default();
            let key = order.order_id.to_string();
            let record = FutureRecord::to("validated_orders")
                .payload(&payload)
                .key(&key);
            let _ = producer.send(record, Duration::from_secs(0));
        }

        let total_us = start.elapsed().as_micros();

        // Profile logging
        if self.stats.received % 100 == 0 {
            log::info!(
                "[PROFILE] parse={}µs validate={}µs wal_append={}µs wal_flush={}µs total={}µs",
                t_parse.as_micros(),
                (t_validate - t_parse).as_micros(),
                wal_append_us,
                wal_flush_us,
                total_us
            );
        }

        // Track latency
        self.track_latency(total_us as u64);

        self.accept_response(order.order_id)
    }

    /// Handle CANCEL message (stub for future)
    fn handle_cancel(&mut self, _body: &[u8]) -> Vec<u8> {
        // TODO: Implement cancel order
        log::warn!("[UBSC] Cancel not implemented yet");
        self.error_response(0, reason_codes::INTERNAL_ERROR)
    }

    /// Handle QUERY message (stub for future)
    fn handle_query(&mut self, _body: &[u8]) -> Vec<u8> {
        // TODO: Implement query
        log::warn!("[UBSC] Query not implemented yet");
        self.error_response(0, reason_codes::INTERNAL_ERROR)
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

    fn track_latency(&mut self, total_us: u64) {
        self.stats.latency_sum_us += total_us;
        if total_us < self.stats.latency_min_us {
            self.stats.latency_min_us = total_us;
        }
        if total_us > self.stats.latency_max_us {
            self.stats.latency_max_us = total_us;
        }
    }
}
