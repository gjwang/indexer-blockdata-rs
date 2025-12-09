//! UBSCore Handler - Business Logic Only
//!
//! This module contains the order processing business logic,
//! completely separated from transport (Aeron).
//!
//! The handler receives order bytes and returns response bytes.
//! It doesn't know about Aeron, correlation IDs, or channels.

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

/// Order processing result
pub enum ProcessResult {
    Accept { order_id: u64 },
    Reject { order_id: u64, reason_code: u8 },
    Invalid,
}

/// UBSCore order handler - business logic only
pub struct UbsCoreHandler<'a> {
    pub ubs_core: &'a mut UBSCore<SpotRiskModel>,
    pub wal: &'a mut MmapWal,
    pub kafka_producer: &'a Option<FutureProducer>,
    pub stats: &'a mut HandlerStats,
}

impl<'a> UbsCoreHandler<'a> {
    /// Process raw order bytes and return response bytes
    ///
    /// This is the ONLY interface - bytes in, bytes out.
    /// No transport concerns here.
    pub fn process(&mut self, order_payload: &[u8]) -> Vec<u8> {
        let start = Instant::now();
        self.stats.received += 1;

        // Parse order message
        let order_msg = match OrderMessage::from_bytes(order_payload) {
            Some(msg) => msg,
            None => {
                log::warn!("[UBSC] Invalid order message");
                return Vec::new(); // Empty = invalid
            }
        };

        // Convert to InternalOrder
        let order = match order_msg.to_internal_order() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("[UBSC] Order conversion failed: {:?}", e);
                self.stats.rejected += 1;
                return ResponseMessage::reject(order_msg.order_id, reason_codes::INVALID_SYMBOL)
                    .to_bytes()
                    .to_vec();
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
            return ResponseMessage::reject(order.order_id, reason_code)
                .to_bytes()
                .to_vec();
        }
        let t_validate = start.elapsed();

        // 2. WAL APPEND
        let t_wal_append_start = Instant::now();
        if let Ok(payload) = bincode::serialize(&order) {
            let entry = WalEntry::new(WalEntryType::OrderLock, payload);
            if let Err(e) = self.wal.append(&entry) {
                log::error!("[UBSC] WAL append failed: {:?}", e);
                self.stats.rejected += 1;
                return ResponseMessage::reject(order.order_id, reason_codes::INTERNAL_ERROR)
                    .to_bytes()
                    .to_vec();
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
        self.stats.latency_sum_us += total_us as u64;
        if (total_us as u64) < self.stats.latency_min_us {
            self.stats.latency_min_us = total_us as u64;
        }
        if (total_us as u64) > self.stats.latency_max_us {
            self.stats.latency_max_us = total_us as u64;
        }

        ResponseMessage::accept(order.order_id).to_bytes().to_vec()
    }
}
