//! UBSCore Handler - PURE Business Logic with Batch Support
//!
//! Supports both single-message and batch processing.
//! Batch mode: append all → single flush → respond all

use std::time::Duration;

use rdkafka::producer::{FutureProducer, FutureRecord};

use crate::ubs_core::{
    MmapWal, RejectReason, SpotRiskModel, UBSCore,
    WalEntry, WalEntryType,
};
use crate::ubs_core::comm::{
    OrderMessage, DepositMessage, WithdrawMessage, ResponseMessage, reason_codes, WireMessage,
};
use crate::ubs_core::order::InternalOrder;
use super::message::{MsgType, parse_message, build_message};

/// Pending order after validation and WAL append (before flush)
struct PendingOrder {
    order: InternalOrder,
    response: Vec<u8>,
}

/// Result of processing a single message (before flush)
enum ProcessResult {
    /// Order accepted, WAL appended, waiting for flush
    Pending(PendingOrder),
    /// Immediate response (reject, deposit, withdraw, etc.)
    Immediate(Vec<u8>),
}

/// UBSCore message handler - PURE business logic
pub struct UbsCoreHandler<'a> {
    pub ubs_core: &'a mut UBSCore<SpotRiskModel>,
    pub wal: &'a mut MmapWal,
    pub kafka_producer: &'a Option<FutureProducer>,
}

impl<'a> UbsCoreHandler<'a> {
    // ========== BATCH PROCESSING API ==========

    /// Process a batch of messages with single WAL flush
    ///
    /// Flow:
    /// 1. For each message: validate, append to WAL (no flush)
    /// 2. Single flush for all
    /// 3. Forward all to Kafka, return responses
    pub fn process_batch(&mut self, items: &[(u64, Vec<u8>)]) -> Vec<(u64, Vec<u8>)> {
        use std::time::Instant;

        let batch_start = Instant::now();
        let batch_size = items.len();

        let mut results: Vec<(u64, ProcessResult)> = Vec::with_capacity(batch_size);
        let mut pending_count = 0;

        // Phase 1: Validate + Append (no flush)
        let phase1_start = Instant::now();
        for (correlation_id, payload) in items {
            let result = self.process_no_flush(payload);
            if matches!(result, ProcessResult::Pending(_)) {
                pending_count += 1;
            }
            results.push((*correlation_id, result));
        }
        let phase1_us = phase1_start.elapsed().as_micros();

        // Phase 2: Single flush
        let phase2_start = Instant::now();
        if pending_count > 0 {
            if let Err(e) = self.wal.flush() {
                log::error!("[UBSC] Batch WAL flush failed: {:?}", e);
            }
        }
        let phase2_us = phase2_start.elapsed().as_micros();

        // Phase 3: Forward to Kafka + build responses
        let phase3_start = Instant::now();
        let mut responses = Vec::with_capacity(results.len());
        for (correlation_id, result) in results {
            let response = match result {
                ProcessResult::Pending(pending) => {
                    self.forward_to_kafka(&pending.order);
                    pending.response
                }
                ProcessResult::Immediate(response) => response,
            };
            responses.push((correlation_id, response));
        }
        let phase3_us = phase3_start.elapsed().as_micros();

        let total_us = batch_start.elapsed().as_micros();

        // Profile log (file only, not stdout)
        if batch_size > 0 {
            let ops = if total_us > 0 {
                (batch_size as u128 * 1_000_000) / total_us
            } else {
                0
            };
            log::info!(
                "[PROFILE] batch={} pending={} validate_append={}µs flush={}µs kafka={}µs total={}µs ops={}",
                batch_size, pending_count, phase1_us, phase2_us, phase3_us, total_us, ops
            );
        }

        responses
    }

    /// Process single message without flush (for batch mode)
    fn process_no_flush(&mut self, payload: &[u8]) -> ProcessResult {
        let (msg_type, body) = match parse_message(payload) {
            Some(r) => r,
            None => {
                log::warn!("[UBSC] Empty message");
                return ProcessResult::Immediate(self.error_response(0, reason_codes::INTERNAL_ERROR));
            }
        };

        match msg_type {
            MsgType::Order => self.handle_order_no_flush(body),
            MsgType::Cancel => ProcessResult::Immediate(self.handle_cancel(body)),
            MsgType::Query => ProcessResult::Immediate(self.handle_query(body)),
            MsgType::Deposit => ProcessResult::Immediate(self.handle_deposit(body)),
            MsgType::Withdraw => ProcessResult::Immediate(self.handle_withdraw(body)),
            MsgType::Response => {
                log::warn!("[UBSC] Unexpected response message");
                ProcessResult::Immediate(self.error_response(0, reason_codes::INTERNAL_ERROR))
            }
        }
    }

    /// Handle ORDER without flush (for batch)
    fn handle_order_no_flush(&mut self, body: &[u8]) -> ProcessResult {
        let order_msg = match OrderMessage::from_bytes(body) {
            Some(msg) => msg,
            None => {
                log::warn!("[UBSC] Invalid order message");
                return ProcessResult::Immediate(self.error_response(0, reason_codes::INTERNAL_ERROR));
            }
        };

        let order = match order_msg.to_internal_order() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("[UBSC] Order conversion failed: {:?}", e);
                return ProcessResult::Immediate(self.reject_response(order_msg.order_id, reason_codes::INVALID_SYMBOL));
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
            return ProcessResult::Immediate(self.reject_response(order.order_id, reason_code));
        }

        // 2. WAL APPEND (no flush - done in batch)
        if let Ok(payload) = bincode::serialize(&order) {
            let entry = WalEntry::new(WalEntryType::OrderLock, payload);
            if let Err(e) = self.wal.append(&entry) {
                log::error!("[UBSC] WAL append failed: {:?}", e);
                return ProcessResult::Immediate(self.reject_response(order.order_id, reason_codes::INTERNAL_ERROR));
            }
        }

        // Return pending (will be completed after batch flush)
        ProcessResult::Pending(PendingOrder {
            order,
            response: self.accept_response(order_msg.order_id),
        })
    }

    /// Forward order to Kafka (called after flush)
    fn forward_to_kafka(&self, order: &InternalOrder) {
        if let Some(producer) = self.kafka_producer {
            let payload = bincode::serialize(order).unwrap_or_default();
            let key = order.order_id.to_string();
            let record = FutureRecord::to("validated_orders")
                .payload(&payload)
                .key(&key);
            let _ = producer.send(record, Duration::from_secs(0));
        }
    }

    // ========== SINGLE MESSAGE API (kept for compatibility) ==========

    /// Process single message with immediate flush
    pub fn on_message(&mut self, payload: &[u8]) -> Vec<u8> {
        let (msg_type, body) = match parse_message(payload) {
            Some(r) => r,
            None => {
                log::warn!("[UBSC] Empty message");
                return self.error_response(0, reason_codes::INTERNAL_ERROR);
            }
        };

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

    /// Handle ORDER message (single message with flush)
    fn handle_order(&mut self, body: &[u8]) -> Vec<u8> {
        let order_msg = match OrderMessage::from_bytes(body) {
            Some(msg) => msg,
            None => {
                log::warn!("[UBSC] Invalid order message");
                return self.error_response(0, reason_codes::INTERNAL_ERROR);
            }
        };

        let order = match order_msg.to_internal_order() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("[UBSC] Order conversion failed: {:?}", e);
                return self.reject_response(order_msg.order_id, reason_codes::INVALID_SYMBOL);
            }
        };

        if let Err(reason) = self.ubs_core.validate_order(&order) {
            let reason_code = match reason {
                RejectReason::InsufficientBalance => reason_codes::INSUFFICIENT_BALANCE,
                RejectReason::DuplicateOrderId => reason_codes::DUPLICATE_ORDER_ID,
                RejectReason::AccountNotFound => reason_codes::ACCOUNT_NOT_FOUND,
                _ => reason_codes::INTERNAL_ERROR,
            };
            return self.reject_response(order.order_id, reason_code);
        }

        if let Ok(payload) = bincode::serialize(&order) {
            let entry = WalEntry::new(WalEntryType::OrderLock, payload);
            if let Err(e) = self.wal.append(&entry) {
                log::error!("[UBSC] WAL append failed: {:?}", e);
                return self.reject_response(order.order_id, reason_codes::INTERNAL_ERROR);
            }
        }

        if let Err(e) = self.wal.flush() {
            log::error!("[UBSC] WAL flush failed: {:?}", e);
        }

        self.forward_to_kafka(&order);
        self.accept_response(order.order_id)
    }

    fn handle_cancel(&mut self, body: &[u8]) -> Vec<u8> {
        // Parse cancel message: [user_id: 8 bytes][order_id: 8 bytes]
        if body.len() < 16 {
            log::warn!("[UBSC] Invalid cancel message length: {}", body.len());
            return self.error_response(0, reason_codes::INTERNAL_ERROR);
        }

        let user_id = u64::from_le_bytes(body[0..8].try_into().unwrap());
        let order_id = u64::from_le_bytes(body[8..16].try_into().unwrap());

        log::info!("[UBSC] Cancel request: user={} order={}", user_id, order_id);

        // Forward cancel to ME via Kafka
        if let Some(producer) = self.kafka_producer {
            use crate::models::OrderRequest;

            // Note: symbol_id and checksum are not used by ME for cancel
            // but required by the OrderRequest enum structure
            let cancel_request = OrderRequest::CancelOrder {
                user_id,
                order_id,
                symbol_id: 0, // Placeholder - ME ignores this for cancel
                checksum: 0,  // Placeholder - ME ignores this for cancel
            };

            if let Ok(payload) = bincode::serialize(&cancel_request) {
                let key = order_id.to_string();
                let record = FutureRecord::to("validated_orders")
                    .payload(&payload)
                    .key(&key);

                // Send async (fire and forget)
                let _ = producer.send(record, Duration::from_secs(0));
                log::info!("[UBSC] Cancel forwarded to ME: order={}", order_id);
                return self.accept_response(order_id);
            }
        }

        log::warn!("[UBSC] No Kafka producer for cancel");
        self.error_response(order_id, reason_codes::INTERNAL_ERROR)
    }

    fn handle_query(&mut self, _body: &[u8]) -> Vec<u8> {
        log::warn!("[UBSC] Query not implemented yet");
        self.error_response(0, reason_codes::INTERNAL_ERROR)
    }

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
