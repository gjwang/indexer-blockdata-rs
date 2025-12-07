use fetcher::configure;
use fetcher::db::{OrderHistoryDb, SettlementDb};
use fetcher::ledger::{OrderStatus, OrderUpdate};
use fetcher::engine_output::{EngineOutput, InputData, GENESIS_HASH};
use fetcher::logger::setup_logger;
use fetcher::starrocks_client::{StarRocksClient, StarRocksTrade};
use zmq::{Context, PULL};
use chrono::Utc;
use tokio::sync::mpsc;
use std::sync::Arc;

const LOG_TARGET: &str = "settlement";

#[tokio::main]
async fn main() {
    // Load service-specific configuration (config/settlement_config.yaml)
    let config = configure::load_service_config("settlement_config")
        .expect("Failed to load settlement configuration");

    // Setup logger
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    // Get configurations
    let zmq_config = config.zeromq.expect("ZMQ config missing");
    let scylla_config = config.scylladb.expect("ScyllaDB config missing");

    // Initialize ScyllaDB connections
    log::info!(target: LOG_TARGET, "Connecting to ScyllaDB (Settlement)...");
    let settlement_db = match SettlementDb::connect(&scylla_config).await {
        Ok(db) => {
            log::info!(target: LOG_TARGET, "✅ Connected to ScyllaDB (Settlement)");
            std::sync::Arc::new(db)
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to Settlement DB: {}", e);
            return;
        }
    };

    log::info!(target: LOG_TARGET, "Connecting to ScyllaDB (OrderHistory)...");
    let order_history_db = match OrderHistoryDb::connect(&scylla_config).await {
        Ok(db) => {
            log::info!(target: LOG_TARGET, "✅ Connected to ScyllaDB (OrderHistory)");
            std::sync::Arc::new(db)
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to OrderHistory DB: {}", e);
            return;
        }
    };

    // Initialize StarRocks Client
    let starrocks_client = std::sync::Arc::new(StarRocksClient::new());

    // Health Checks
    if let Err(e) = settlement_db.health_check().await {
        log::error!(target: LOG_TARGET, "SettlementDB health check failed: {}", e);
    }
    if let Err(e) = order_history_db.health_check().await {
        log::error!(target: LOG_TARGET, "OrderHistoryDB health check failed: {}", e);
    }

    // Setup ZMQ PULL socket for data (BIND as server)
    let context = Context::new();
    let subscriber = context.socket(PULL).expect("Failed to create PULL socket");
    subscriber.set_rcvhwm(1_000_000).expect("Failed to set RCVHWM");
    let endpoint = format!("tcp://*:{}", zmq_config.settlement_port);
    subscriber.bind(&endpoint).expect("Failed to bind to settlement port");
    log::info!(target: LOG_TARGET, "📡 Listening on tcp://localhost:{}", zmq_config.settlement_port);

    // Setup REP socket for synchronization (tell ME we're ready)
    let sync_socket = context.socket(zmq::REP).expect("Failed to create REP socket");
    let sync_port = zmq_config.settlement_port + 2; // 5559
    sync_socket.bind(&format!("tcp://*:{}", sync_port)).expect("Failed to bind sync socket");
    log::info!(target: LOG_TARGET, "🔄 Sync socket bound on port {}", sync_port);

    // Send READY signal in a separate thread (non-blocking)
    std::thread::spawn(move || {
        log::info!(target: LOG_TARGET, "⏳ Waiting for ME to request READY signal...");
        if let Ok(msg) = sync_socket.recv_bytes(0) {
            if msg == b"PING" {
                sync_socket.send(&b"READY"[..], 0).expect("Failed to send READY");
                log::info!(target: LOG_TARGET, "✅ Sent READY signal to ME");
            }
        }
    });

    // File logging setup (CSV/Failed Trades)
    let data_dir_str = configure::prepare_data_dir(&config.data_dir).unwrap_or_else(|e| {
        log::error!(target: LOG_TARGET, "Data dir error: {}", e);
        "./data".to_string()
    });
    let data_dir = std::path::PathBuf::from(data_dir_str);

    let backup_csv_file = if config.backup_csv_file.is_empty() {
        data_dir.join("settled_trades.csv")
    } else {
        data_dir.join(&config.backup_csv_file)
    };

    let failed_trades_file = if config.failed_trades_file.is_empty() {
        data_dir.join("failed_trades.json")
    } else {
        data_dir.join(&config.failed_trades_file)
    };

    log::info!(target: LOG_TARGET, "✅ Settlement Service Started");
    log::info!(target: LOG_TARGET, "📡 Listening on {}", endpoint);
    log::info!(target: LOG_TARGET, "📂 csv={} failed={}", backup_csv_file.display(), failed_trades_file.display());

    let mut total_settled: u64 = 0;
    let mut total_errors: u64 = 0;
    let _total_history_events = 0u64;

    // Chain verification state - LOAD FROM DB for crash recovery
    let (mut last_output_seq, mut last_processed_hash) = match settlement_db.get_chain_state().await {
        Ok((seq, hash)) => {
            if seq > 0 {
                log::info!(target: LOG_TARGET, "🔄 Recovered chain state: seq={} hash={}", seq, hash);
            }
            (seq, hash)
        }
        Err(e) => {
            log::warn!(target: LOG_TARGET, "⚠️ Failed to load chain state (using defaults): {}", e);
            (0u64, GENESIS_HASH)
        }
    };

    // CSV Writer
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(&backup_csv_file)
        .expect("Failed to open backup CSV file");
    let mut wtr = csv::WriterBuilder::new().has_headers(false).from_writer(file);

    // Reorder buffer for out-of-order ZMQ messages
    // Key: output_seq, Value: EngineOutput
    let mut reorder_buffer: std::collections::BTreeMap<u64, EngineOutput> = std::collections::BTreeMap::new();
    const MAX_BUFFER_SIZE: usize = 100_000; // Very large - should NEVER hit this with proper back-pressure

    let mut msg_count = 0u64;

    // === DERIVED WRITERS: Bounded Channels for Ordered Processing ===
    // Each writer has its own channel with back-pressure when buffer is full
    const DERIVED_WRITER_BUFFER_SIZE: usize = 10_000;

    // Balance events channel
    let (balance_tx, mut balance_rx) = mpsc::channel::<Vec<fetcher::engine_output::BalanceEvent>>(DERIVED_WRITER_BUFFER_SIZE);
    let sdb_balance = settlement_db.clone();
    tokio::spawn(async move {
        while let Some(events) = balance_rx.recv().await {
            if let Err(e) = sdb_balance.append_balance_events_batch(&events).await {
                log::error!(target: "settlement", "Balance events write failed (can replay): {}", e);
            }
        }
    });

    // Trades channel
    let (trades_tx, mut trades_rx) = mpsc::channel::<(Vec<fetcher::engine_output::TradeOutput>, u64)>(DERIVED_WRITER_BUFFER_SIZE);
    let sdb_trades = settlement_db.clone();
    tokio::spawn(async move {
        while let Some((trades, seq)) = trades_rx.recv().await {
            if let Err(e) = sdb_trades.insert_trades_batch(&trades, seq).await {
                log::error!(target: "settlement", "Trades write failed (can replay): {}", e);
            }
        }
    });

    // Order updates channel
    let (order_tx, mut order_rx) = mpsc::channel::<EngineOutput>(DERIVED_WRITER_BUFFER_SIZE);
    let ohdb_orders = order_history_db.clone();
    tokio::spawn(async move {
        while let Some(output) = order_rx.recv().await {
            if let Err(e) = process_engine_order_update(&ohdb_orders, &output).await {
                log::error!(target: "settlement", "Order update write failed (can replay): {}", e);
            }
        }
    });

    // StarRocks trades channel
    let (starrocks_tx, mut starrocks_rx) = mpsc::channel::<Vec<StarRocksTrade>>(DERIVED_WRITER_BUFFER_SIZE);
    let sr_client = starrocks_client.clone();
    tokio::spawn(async move {
        while let Some(trades) = starrocks_rx.recv().await {
            for trade in trades {
                if let Err(e) = sr_client.load_trade(trade).await {
                    log::error!(target: "settlement", "StarRocks trade load failed: {}", e);
                }
            }
        }
    });

    log::info!(target: LOG_TARGET, "✅ Derived writers started (buffer_size={})", DERIVED_WRITER_BUFFER_SIZE);

    // === ULTRA-FAST BATCH PROCESSING ===
    const MAX_BATCH_SIZE: usize = 100;  // Max outputs to batch together

    // Progress tracking
    let start_time = std::time::Instant::now();
    let mut total_msgs: u64 = 0;
    let mut total_batches: u64 = 0;
    let mut total_processing_us: u64 = 0;  // Only time spent processing (excludes ZMQ wait)

    loop {
        let mut batch: Vec<EngineOutput> = Vec::with_capacity(MAX_BATCH_SIZE);

        // === Step 1: Receive first message (blocking) ===
        msg_count += 1;
        let data = match subscriber.recv_bytes(0) {
            Ok(d) => d,
            Err(e) => {
                log::error!(target: LOG_TARGET, "ZMQ recv error: {}", e);
                continue;
            }
        };

        match serde_json::from_slice::<EngineOutput>(&data) {
            Ok(engine_output) => batch.push(engine_output),
            Err(e) => {
                log::warn!(target: LOG_TARGET, "Invalid EngineOutput: {}", e);
                continue;
            }
        }

        // === Step 2: Drain more messages (non-blocking) to fill batch ===
        while batch.len() < MAX_BATCH_SIZE {
            match subscriber.recv_bytes(zmq::DONTWAIT) {
                Ok(data) => {
                    msg_count += 1;
                    if let Ok(engine_output) = serde_json::from_slice::<EngineOutput>(&data) {
                        batch.push(engine_output);
                    }
                }
                Err(_) => break, // No more messages available
            }
        }

        // === Step 3: Sort batch by sequence (handle out-of-order) ===
        batch.sort_by_key(|o| o.output_seq);

        // === Step 4: Filter and verify batch ===
        let mut verified_batch: Vec<EngineOutput> = Vec::with_capacity(batch.len());
        let mut had_error = false;

        for output in batch.drain(..) {
            let seq = output.output_seq;
            let expected_seq = last_output_seq + 1;

            if seq < expected_seq {
                // Already processed (idempotency)
                continue;
            } else if seq > expected_seq {
                // Future message - buffer it for later
                if reorder_buffer.len() < MAX_BUFFER_SIZE {
                    reorder_buffer.insert(seq, output);
                }
                continue;
            }

            // seq == expected_seq - verify and add to batch
            match verify_engine_output(&output, last_output_seq, last_processed_hash) {
                VerifyResult::Skip => continue,
                VerifyResult::Err(msg) => {
                    log::error!(target: LOG_TARGET, "Verification failed: {}", msg);
                    had_error = true;
                    break;
                }
                VerifyResult::Ok => {
                    // Update chain state for next iteration
                    last_processed_hash = output.hash;
                    last_output_seq = output.output_seq;
                    verified_batch.push(output);
                }
            }

            // Drain any buffered messages that are now in order
            while let Some((&next_seq, _)) = reorder_buffer.first_key_value() {
                if next_seq == last_output_seq + 1 {
                    let buffered = reorder_buffer.remove(&next_seq).unwrap();
                    match verify_engine_output(&buffered, last_output_seq, last_processed_hash) {
                        VerifyResult::Skip => continue,
                        VerifyResult::Err(msg) => {
                            log::error!(target: LOG_TARGET, "Buffered verification failed: {}", msg);
                            had_error = true;
                            break;
                        }
                        VerifyResult::Ok => {
                            last_processed_hash = buffered.hash;
                            last_output_seq = buffered.output_seq;
                            verified_batch.push(buffered);
                        }
                    }
                } else {
                    break;
                }
            }

            if had_error {
                break;
            }
        }

        if verified_batch.is_empty() {
            continue;
        }

        // === Step 5: Batch write to Source of Truth (ULTRA FAST) ===
        let t_batch = std::time::Instant::now();
        let batch_size = verified_batch.len();

        if let Err(e) = settlement_db.write_engine_outputs_batch(&verified_batch).await {
            log::error!(target: LOG_TARGET, "❌ Batch SOT write failed: {}", e);
            // On failure, we should retry (for now just log and continue)
            total_errors += batch_size as u64;
            continue;
        }
        let d_sot = t_batch.elapsed();

        // === Step 6: Dispatch to Derived Writers ===
        let t_queue = std::time::Instant::now();
        for output in &verified_batch {
            // Balance events
            if !output.balance_events.is_empty() {
                let _ = balance_tx.send(output.balance_events.clone()).await;
            }
            // Trades
            if !output.trades.is_empty() {
                let _ = trades_tx.send((output.trades.clone(), output.output_seq)).await;
            }
            // Order updates
            let _ = order_tx.send(output.clone()).await;
            // StarRocks trades
            if !output.trades.is_empty() {
                let sr_trades: Vec<StarRocksTrade> = output.trades.iter().map(|trade| {
                    let ts = if trade.settled_at > 0 { trade.settled_at as i64 } else { Utc::now().timestamp_millis() };
                    let match_exec = fetcher::ledger::MatchExecData {
                        trade_id: trade.trade_id,
                        buy_order_id: trade.buy_order_id,
                        sell_order_id: trade.sell_order_id,
                        buyer_user_id: trade.buyer_user_id,
                        seller_user_id: trade.seller_user_id,
                        price: trade.price,
                        quantity: trade.quantity,
                        base_asset_id: trade.base_asset_id,
                        quote_asset_id: trade.quote_asset_id,
                        buyer_refund: trade.buyer_refund,
                        seller_refund: trade.seller_refund,
                        match_seq: trade.match_seq,
                        output_sequence: output.output_seq,
                        settled_at: trade.settled_at,
                        buyer_quote_version: 0,
                        buyer_base_version: 0,
                        seller_base_version: 0,
                        seller_quote_version: 0,
                        buyer_quote_balance_after: 0,
                        buyer_base_balance_after: 0,
                        seller_base_balance_after: 0,
                        seller_quote_balance_after: 0,
                    };
                    let _ = wtr.serialize(&match_exec);
                    StarRocksTrade::from_match_exec(&match_exec, ts)
                }).collect();
                let _ = starrocks_tx.send(sr_trades).await;
            }
        }
        let d_queue = t_queue.elapsed();
        let _ = wtr.flush();

        // === PROFILING ===
        let last_seq = verified_batch.last().map(|o| o.output_seq).unwrap_or(0);
        let batch_trades: usize = verified_batch.iter().map(|o| o.trades.len()).sum();

        // Update running totals
        total_msgs += batch_size as u64;
        total_batches += 1;
        total_settled += batch_trades as u64;

        // Track processing time (sot + queue, excludes ZMQ wait)
        let d_batch = d_sot + d_queue;
        total_processing_us += d_batch.as_micros() as u64;

        // Calculate throughput
        let batch_ops = if d_batch.as_micros() > 0 {
            (batch_size as f64 * 1_000_000.0) / d_batch.as_micros() as f64
        } else { 0.0 };

        // TRUE msg throughput = total msgs / processing time only (not wall clock)
        let msg_ops = if total_processing_us > 0 {
            (total_msgs as f64 * 1_000_000.0) / total_processing_us as f64
        } else { 0.0 };

        let buffer_len = reorder_buffer.len();

        log::info!(target: LOG_TARGET,
            "[PROGRESS] seq={} msgs={} trades={} | batch: n={} {:.0}op/s | total: {:.0}msg/s | buf={}",
            last_seq, total_msgs, total_settled,
            batch_size, batch_ops,
            msg_ops, buffer_len);
    }
}

// Helper functions (copied from order_history_service.rs)
async fn process_order_update(
    db: &OrderHistoryDb,
    order_update: &OrderUpdate,
    event_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    match order_update.status {
        OrderStatus::New => {
            db.upsert_active_order(order_update).await?;
            db.insert_order_history(order_update).await?;
            db.insert_order_update_stream(order_update, event_id).await?;
            db.init_user_statistics(order_update.user_id, order_update.timestamp).await?;
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;
        }
        OrderStatus::PartiallyFilled => {
            db.upsert_active_order(order_update).await?;
            db.insert_order_history(order_update).await?;
            db.insert_order_update_stream(order_update, event_id).await?;
        }
        OrderStatus::Filled | OrderStatus::Cancelled | OrderStatus::Expired => {
            db.delete_active_order(order_update.user_id, order_update.order_id).await?;
            db.insert_order_history(order_update).await?;
            db.insert_order_update_stream(order_update, event_id).await?;
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;
        }
        OrderStatus::Rejected => {
            db.insert_order_history(order_update).await?;
            db.insert_order_update_stream(order_update, event_id).await?;
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;
        }
    }
    Ok(())
}

/// Process order update from EngineOutput (if present)
async fn process_engine_order_update(
    db: &OrderHistoryDb,
    output: &EngineOutput,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let InputData::PlaceOrder(ref place_order) = output.input.data {
        if let Some(ref order_update) = output.order_update {
            let status = match order_update.status {
                1 => OrderStatus::New,
                2 => OrderStatus::PartiallyFilled,
                3 => OrderStatus::Filled,
                4 => OrderStatus::Cancelled,
                _ => OrderStatus::Rejected,
            };

            let ledger_order_update = OrderUpdate {
                order_id: order_update.order_id,
                client_order_id: Some(place_order.cid.clone()),
                user_id: order_update.user_id,
                symbol_id: place_order.symbol_id,
                side: place_order.side,
                order_type: place_order.order_type,
                status,
                price: place_order.price,
                qty: place_order.quantity,
                filled_qty: order_update.filled_qty,
                avg_fill_price: if order_update.avg_price > 0 { Some(order_update.avg_price) } else { None },
                rejection_reason: None,
                timestamp: order_update.updated_at,
                match_id: None,
            };

            if let Err(e) = process_order_update(db, &ledger_order_update, 0).await {
                log::warn!("Order history update failed: {}", e);
            } else {
                log::info!(target: LOG_TARGET, "Processed Order #{}", ledger_order_update.order_id);
            }
        }
    }
    Ok(())
}

/// Result of engine output verification
enum VerifyResult {
    /// Already processed, skip
    Skip,
    /// Verification passed
    Ok,
    /// Verification failed with error message
    Err(String),
}

/// Verify EngineOutput chain integrity and idempotency
fn verify_engine_output(
    output: &EngineOutput,
    last_output_seq: u64,
    last_processed_hash: u64,
) -> VerifyResult {
    // 1. Idempotency: Skip if already processed
    if output.output_seq <= last_output_seq && last_output_seq > 0 {
        return VerifyResult::Skip;
    }

    // 2. Hash chain verification
    if !output.verify_prev_hash(last_processed_hash) {
        return VerifyResult::Err(format!(
            "Chain verification failed: expected prev_hash={}, got={}",
            last_processed_hash, output.prev_hash
        ));
    }

    // 3. Output integrity verification
    if !output.verify() {
        return VerifyResult::Err(format!(
            "Output hash verification failed for seq={}",
            output.output_seq
        ));
    }

    // 4. Sequence gap warning (not an error, just a warning)
    if output.output_seq != last_output_seq + 1 {
        log::warn!(target: LOG_TARGET,
            "Sequence gap: expected {}, got {}", last_output_seq + 1, output.output_seq);
    }

    VerifyResult::Ok
}
