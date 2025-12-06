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
    const MAX_BUFFER_SIZE: usize = 100_000; // Very large - should NEVER hit this with proper back-pressure

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
                        // Buffer full - this should NEVER happen with proper back-pressure
                        // Log critical error but still buffer (we can't lose messages)
                        log::error!(target: LOG_TARGET,
                            "🚨 CRITICAL: Reorder buffer full ({} messages)! Expected seq={}, got seq={}. Investigate ZMQ back-pressure!",
                            reorder_buffer.len(), expected_seq, seq
                        );
                        // Still insert - we'd rather use more memory than lose messages
                        reorder_buffer.insert(seq, engine_output);
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

    // 2-4. Verification (hash chain, output integrity, sequence)
    let t_verify = std::time::Instant::now();

    if !output.verify_prev_hash(*last_processed_hash) {
        return Err(format!(
            "Chain verification failed: expected prev_hash={}, got={}",
            *last_processed_hash, output.prev_hash
        ).into());
    }

    if !output.verify() {
        return Err(format!(
            "Output hash verification failed for seq={}",
            output.output_seq
        ).into());
    }

    if output.output_seq != *last_output_seq + 1 {
        log::warn!(target: LOG_TARGET,
            "Sequence gap: expected {}, got {}", *last_output_seq + 1, output.output_seq);
    }
    let d_verify = t_verify.elapsed();

    // === PARALLEL DB WRITES ===
    // All these write to different tables, so can run concurrently
    let t_db = std::time::Instant::now();

    let output_seq = output.output_seq;
    let output_hash = output.hash;

    // Run ALL DB writes in parallel (including chain state persist)
    let (log_result, bal_result, trade_result, persist_result) = tokio::join!(
        settlement_db.write_engine_output(output),
        settlement_db.append_balance_events_batch(&output.balance_events),
        settlement_db.insert_trades_batch(&output.trades, output_seq),
        settlement_db.save_chain_state(output_seq, output_hash),
    );

    // Check results
    log_result.map_err(|e| format!("Failed to write engine_output: {}", e))?;
    bal_result.map_err(|e| format!("Balance events batch failed: {}", e))?;
    trade_result.map_err(|e| format!("Trades batch insert failed: {}", e))?;
    persist_result.map_err(|e| format!("Failed to persist chain state: {}", e))?;

    let d_db = t_db.elapsed(); // Combined time for all parallel ops

    // Update in-memory chain state after successful persist
    *last_processed_hash = output_hash;
    *last_output_seq = output_seq;

    // 6. StarRocks (async, fire-and-forget for all trades)
    let t_trade = std::time::Instant::now();
    for trade in &output.trades {
        let sr_client = starrocks_client.clone();
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
        // CSV (non-critical, can fail silently)
        let _ = csv_writer.serialize(&match_exec);

        let sr_trade = StarRocksTrade::from_match_exec(&match_exec, ts);
        tokio::spawn(async move { let _ = sr_client.load_trade(sr_trade).await; });
    }

    // Order fills - do in background (non-critical for settlement)
    let ohs_db = order_history_db.clone();
    let trades_for_fills: Vec<_> = output.trades.iter().map(|t| (t.buyer_user_id, t.buy_order_id, t.seller_user_id, t.sell_order_id, t.quantity)).collect();
    tokio::spawn(async move {
        for (buyer_id, buy_oid, seller_id, sell_oid, qty) in trades_for_fills {
            let _ = process_trade_fill(&ohs_db, buyer_id, buy_oid, qty, 0).await;
            let _ = process_trade_fill(&ohs_db, seller_id, sell_oid, qty, 0).await;
        }
    });

    if !output.trades.is_empty() {
        log::info!(target: LOG_TARGET, "✅ Recorded {} trades from EngineOutput", output.trades.len());
    }
    let d_trade = t_trade.elapsed();

    // 7. Process order update (if present) - Background (not critical for settlement)
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

            // Spawn to background - order history is "nice to have", not critical
            let ohs_db = order_history_db.clone();
            tokio::spawn(async move {
                if let Err(e) = process_order_update(&ohs_db, &ledger_order_update, 0).await {
                    log::warn!("Order history update failed: {}", e);
                } else {
                    log::info!(target: "settlement", "Processed Fill Order #{}", ledger_order_update.order_id);
                }
            });
        }
    }
    let d_order = t_order.elapsed();

    // 8. Flush CSV
    csv_writer.flush()?;

    let d_total = t_start.elapsed();

    // Log timing
    log::info!(target: LOG_TARGET,
        "[PROFILE] seq={} total={:?} verify={:?} db={:?} trade={:?} order={:?}",
        output_seq, d_total, d_verify, d_db, d_trade, d_order);

    Ok(())
}
