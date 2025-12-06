use fetcher::configure;
use fetcher::db::{OrderHistoryDb, SettlementDb};
use fetcher::ledger::{OrderStatus, OrderUpdate};
use fetcher::engine_output::{EngineOutput, InputData, GENESIS_HASH};
use fetcher::logger::setup_logger;
use fetcher::starrocks_client::{StarRocksClient, StarRocksTrade};
use zmq::{Context, PULL};
use chrono::Utc;

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
    const MAX_BUFFER_SIZE: usize = 1000; // Max buffered messages before force-processing

    let mut msg_count = 0u64;
    loop {
        msg_count += 1;
        log::debug!(target: LOG_TARGET, "🔄 Attempting to receive message #{}", msg_count);

        let data = match subscriber.recv_bytes(0) {
            Ok(d) => {
                log::debug!(target: LOG_TARGET, "✅ Received {} bytes for message #{}", d.len(), msg_count);
                d
            },
            Err(e) => {
                log::error!(target: LOG_TARGET, "ZMQ recv bytes error: {}", e);
                continue;
            }
        };

        // EngineOutput is now the only supported message type
        match serde_json::from_slice::<EngineOutput>(&data) {
            Ok(engine_output) => {
                let seq = engine_output.output_seq;
                let expected_seq = last_output_seq + 1;

                if seq == expected_seq {
                    // In order - process immediately
                    if let Err(e) = process_engine_output(
                        &settlement_db,
                        &order_history_db,
                        &starrocks_client,
                        &engine_output,
                        &mut last_processed_hash,
                        &mut last_output_seq,
                        &mut wtr,
                        msg_count,
                    ).await {
                        total_errors += 1;
                        log::error!(target: LOG_TARGET, "❌ EngineOutput processing failed: {}", e);
                    } else {
                        total_settled += engine_output.trades.len() as u64;
                        log::info!(target: LOG_TARGET,
                            "✅ Processed EngineOutput seq={} trades={} balance_events={}",
                            engine_output.output_seq,
                            engine_output.trades.len(),
                            engine_output.balance_events.len()
                        );
                    }

                    // Now drain any buffered messages that are ready
                    while let Some((&next_seq, _)) = reorder_buffer.first_key_value() {
                        if next_seq == last_output_seq + 1 {
                            let buffered = reorder_buffer.remove(&next_seq).unwrap();
                            if let Err(e) = process_engine_output(
                                &settlement_db,
                                &order_history_db,
                                &starrocks_client,
                                &buffered,
                                &mut last_processed_hash,
                                &mut last_output_seq,
                                &mut wtr,
                                msg_count,
                            ).await {
                                total_errors += 1;
                                log::error!(target: LOG_TARGET, "❌ Buffered EngineOutput processing failed: {}", e);
                                break; // Stop draining on error
                            } else {
                                total_settled += buffered.trades.len() as u64;
                                log::info!(target: LOG_TARGET,
                                    "✅ Processed buffered EngineOutput seq={} trades={} balance_events={}",
                                    buffered.output_seq, buffered.trades.len(), buffered.balance_events.len()
                                );
                            }
                        } else {
                            break; // Gap remains, wait for more messages
                        }
                    }
                } else if seq > expected_seq {
                    // Future message - buffer it
                    if reorder_buffer.len() < MAX_BUFFER_SIZE {
                        log::debug!(target: LOG_TARGET,
                            "📦 Buffering out-of-order seq={} (expected {}), buffer_size={}",
                            seq, expected_seq, reorder_buffer.len() + 1
                        );
                        reorder_buffer.insert(seq, engine_output);
                    } else {
                        // Buffer full - force skip the gap and continue
                        // This prevents getting stuck forever on lost messages
                        log::warn!(target: LOG_TARGET,
                            "⚠️ Buffer full! Force-skipping from seq={} to seq={}. Lost {} messages.",
                            last_output_seq, seq - 1, seq - last_output_seq - 1
                        );
                        // Update state to skip the gap (treat missing as lost)
                        // IMPORTANT: Also reset hash chain to accept the new sequence
                        last_output_seq = seq - 1;
                        last_processed_hash = engine_output.prev_hash; // Accept the incoming chain

                        // Clear obsolete buffered messages (those we've now skipped past)
                        reorder_buffer.retain(|&s, _| s > last_output_seq);

                        // Try to process this message now
                        reorder_buffer.insert(seq, engine_output);
                        // Drain what we can
                        while let Some((&next_seq, _)) = reorder_buffer.first_key_value() {
                            if next_seq == last_output_seq + 1 {
                                let buffered = reorder_buffer.remove(&next_seq).unwrap();
                                if let Err(e) = process_engine_output(
                                    &settlement_db,
                                    &order_history_db,
                                    &starrocks_client,
                                    &buffered,
                                    &mut last_processed_hash,
                                    &mut last_output_seq,
                                    &mut wtr,
                                    msg_count,
                                ).await {
                                    total_errors += 1;
                                    log::error!(target: LOG_TARGET, "❌ Gap-skip EngineOutput processing failed: {}", e);
                                    break;
                                } else {
                                    total_settled += buffered.trades.len() as u64;
                                    log::info!(target: LOG_TARGET,
                                        "✅ Processed gap-skip EngineOutput seq={} trades={} balance_events={}",
                                        buffered.output_seq, buffered.trades.len(), buffered.balance_events.len()
                                    );
                                }
                            } else {
                                break;
                            }
                        }
                    }
                } else {
                    // Past message - already processed, skip (idempotency)
                    log::debug!(target: LOG_TARGET, "Skipping already processed seq={}", seq);
                }
            }
            Err(e) => {
                log::warn!(target: LOG_TARGET, "Unknown message format (not EngineOutput): {}", e);
            }
        }
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

async fn process_trade_fill(
    db: &OrderHistoryDb,
    user_id: u64,
    order_id: u64,
    match_qty: u64,
    event_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(mut order) = db.get_active_order(user_id, order_id).await? {
        order.filled_qty += match_qty;
        if order.filled_qty >= order.qty {
            order.status = OrderStatus::Filled;
        } else {
            order.status = OrderStatus::PartiallyFilled;
        }
        process_order_update(db, &order, event_id).await?;
        log::info!(target: LOG_TARGET, "Processed Fill Order #{}", order_id);
    }
    Ok(())
}

/// Process EngineOutput bundle atomically with chain verification
/// ATOMIC: All operations must succeed, or we fail and retry the whole output
async fn process_engine_output<W: std::io::Write>(
    settlement_db: &std::sync::Arc<SettlementDb>,
    order_history_db: &std::sync::Arc<OrderHistoryDb>,
    starrocks_client: &std::sync::Arc<StarRocksClient>,
    output: &EngineOutput,
    last_processed_hash: &mut u64,
    last_output_seq: &mut u64,
    csv_writer: &mut csv::Writer<W>,
    event_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let t_start = std::time::Instant::now();

    // 1. Idempotency: Skip if already processed
    if output.output_seq <= *last_output_seq && *last_output_seq > 0 {
        log::debug!(target: LOG_TARGET, "Skipping already processed seq={}", output.output_seq);
        return Ok(());
    }

    // 2. Verify chain integrity (hash chain)
    if !output.verify_prev_hash(*last_processed_hash) {
        return Err(format!(
            "Chain verification failed: expected prev_hash={}, got={}",
            *last_processed_hash, output.prev_hash
        ).into());
    }

    // 3. Verify output integrity
    if !output.verify() {
        return Err(format!(
            "Output hash verification failed for seq={}",
            output.output_seq
        ).into());
    }

    // 4. Verify sequence continuity
    if output.output_seq != *last_output_seq + 1 {
        log::warn!(target: LOG_TARGET,
            "Sequence gap: expected {}, got {}", *last_output_seq + 1, output.output_seq);
    }

    // === SOURCE OF TRUTH: Write EngineOutput FIRST ===
    // This is the ONLY authoritative record. All derived state can be rebuilt from this.
    let t_log = std::time::Instant::now();
    settlement_db.write_engine_output(output).await
        .map_err(|e| format!("Failed to write engine_output: {}", e))?;
    let d_log = t_log.elapsed();

    // === DERIVED STATE: All operations below derive from the logged output ===

    // 5. Process balance events - BATCH for performance
    let t_balance = std::time::Instant::now();
    settlement_db.append_balance_events_batch(&output.balance_events).await
        .map_err(|e| format!("Balance events batch failed: {}", e))?;
    let d_balance = t_balance.elapsed();

    // 6. Process trades - FAIL on ANY error
    let t_trade = std::time::Instant::now();
    for trade in &output.trades {
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

        // Insert trade - FAIL on error
        settlement_db.insert_trade(&match_exec).await.map_err(|e|
            format!("Trade insert failed: trade_id={}: {}", trade.trade_id, e)
        )?;

        log::info!(target: LOG_TARGET, "✅ Recorded trade {} from EngineOutput", trade.trade_id);

        // StarRocks (async, non-blocking)
        let sr_client = starrocks_client.clone();
        let ts = if trade.settled_at > 0 { trade.settled_at as i64 } else { Utc::now().timestamp_millis() };
        let sr_trade = StarRocksTrade::from_match_exec(&match_exec, ts);
        tokio::spawn(async move { let _ = sr_client.load_trade(sr_trade).await; });

        // CSV
        csv_writer.serialize(&match_exec)?;

        // Order history fills - FAIL on error
        process_trade_fill(order_history_db, trade.buyer_user_id, trade.buy_order_id, trade.quantity, event_id).await?;
        process_trade_fill(order_history_db, trade.seller_user_id, trade.sell_order_id, trade.quantity, event_id).await?;
    }
    let d_trade = t_trade.elapsed();

    // 7. Process order update (if present) - FAIL on error
    let t_order = std::time::Instant::now();
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

            process_order_update(order_history_db, &ledger_order_update, event_id).await?;
        }
    }
    let d_order = t_order.elapsed();

    // 8. Flush CSV
    csv_writer.flush()?;

    // 9. Update chain state (only after ALL success)
    *last_processed_hash = output.hash;
    *last_output_seq = output.output_seq;

    // 10. PERSIST chain state to DB for crash recovery
    let t_persist = std::time::Instant::now();
    settlement_db.save_chain_state(*last_output_seq, *last_processed_hash).await
        .map_err(|e| format!("Failed to persist chain state: {}", e))?;
    let d_persist = t_persist.elapsed();

    let d_total = t_start.elapsed();

    // Log timing if slow (>50ms) or has trades
    log::info!(target: LOG_TARGET,
        "[PROFILE] seq={} total={:?} log={:?} bal={:?} trade={:?} order={:?} persist={:?}",
        output.output_seq, d_total, d_log, d_balance, d_trade, d_order, d_persist);

    Ok(())
}
