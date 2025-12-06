use fetcher::configure;
use fetcher::db::{OrderHistoryDb, SettlementDb};
use fetcher::ledger::{LedgerCommand, OrderStatus, OrderUpdate};
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
    let mut total_history_events = 0u64;

    // Chain verification state
    let mut last_processed_hash: u64 = GENESIS_HASH;
    let mut last_output_seq: u64 = 0;

    // CSV Writer
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(&backup_csv_file)
        .expect("Failed to open backup CSV file");
    let mut wtr = csv::WriterBuilder::new().has_headers(false).from_writer(file);

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

        // Try EngineOutput first (new flow), then fall back to LedgerCommand
        if let Ok(engine_output) = serde_json::from_slice::<EngineOutput>(&data) {
            // Process EngineOutput with chain verification
            match process_engine_output(
                &settlement_db,
                &order_history_db,
                &starrocks_client,
                &engine_output,
                &mut last_processed_hash,
                &mut last_output_seq,
                &mut wtr,
                msg_count,
            ).await {
                Ok(_) => {
                    total_settled += engine_output.trades.len() as u64;
                    log::info!(target: LOG_TARGET,
                        "✅ Processed EngineOutput seq={} trades={} balance_events={}",
                        engine_output.output_seq,
                        engine_output.trades.len(),
                        engine_output.balance_events.len()
                    );
                }
                Err(e) => {
                    total_errors += 1;
                    log::error!(target: LOG_TARGET, "❌ EngineOutput processing failed: {}", e);
                }
            }
            continue;
        }

        // Fall back to LedgerCommand (balance operations only - trades via EngineOutput)
        match serde_json::from_slice::<LedgerCommand>(&data) {
            Ok(cmd) => {
                match cmd {
                    LedgerCommand::OrderUpdate(order_update) => {
                        if let Err(e) = process_order_update(&order_history_db, &order_update, msg_count).await {
                             log::error!(target: LOG_TARGET, "Order History Update Failed: {}", e);
                        }
                    },
                    // Balance operations via LedgerCommand -> Append-only ledger
                    LedgerCommand::Deposit { user_id, asset_id, amount, balance_after: _, version } => {
                        match settlement_db.deposit(user_id, asset_id, amount, version, 0).await {
                            Ok(bal) => log::info!(target: LOG_TARGET, "Deposit User {} Asset {} -> Seq {}", user_id, asset_id, bal.seq),
                            Err(e) => log::error!(target: LOG_TARGET, "Deposit Failed: {}", e),
                        }
                    },
                    LedgerCommand::Lock { user_id, asset_id, amount, balance_after: _, version } => {
                        match settlement_db.lock(user_id, asset_id, amount, version, 0).await {
                            Ok(bal) => log::info!(target: LOG_TARGET, "Lock User {} Asset {} -> Seq {}", user_id, asset_id, bal.seq),
                            Err(e) => log::error!(target: LOG_TARGET, "Lock Failed: {}", e),
                        }
                    },
                    LedgerCommand::Unlock { user_id, asset_id, amount, balance_after: _, version } => {
                        match settlement_db.unlock(user_id, asset_id, amount, version, 0).await {
                            Ok(bal) => log::info!(target: LOG_TARGET, "Unlock User {} Asset {} -> Seq {}", user_id, asset_id, bal.seq),
                            Err(e) => log::error!(target: LOG_TARGET, "Unlock Failed: {}", e),
                        }
                    },
                    LedgerCommand::Withdraw { user_id, asset_id, amount, balance_after: _, version } => {
                        match settlement_db.withdraw(user_id, asset_id, amount, version, 0).await {
                            Ok(bal) => log::info!(target: LOG_TARGET, "Withdraw User {} Asset {} -> Seq {}", user_id, asset_id, bal.seq),
                            Err(e) => log::error!(target: LOG_TARGET, "Withdraw Failed: {}", e),
                        }
                    },
                    _ => {}
                }
            },
            Err(e) => {
                 log::error!(target: LOG_TARGET, "Failed to deserialize LedgerCommand: {}", e);
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
    // 1. Verify chain integrity
    if !output.verify_prev_hash(*last_processed_hash) {
        return Err(format!(
            "Chain verification failed: expected prev_hash={}, got={}",
            *last_processed_hash, output.prev_hash
        ).into());
    }

    // 2. Verify output integrity
    if !output.verify() {
        return Err(format!(
            "Output hash verification failed for seq={}",
            output.output_seq
        ).into());
    }

    // 3. Verify sequence continuity
    if output.output_seq != *last_output_seq + 1 {
        log::warn!(target: LOG_TARGET,
            "Sequence gap detected: expected {}, got {}",
            *last_output_seq + 1, output.output_seq
        );
        // Allow processing anyway but log warning
    }

    // 4. Process balance events - append to balance_ledger
    // The EngineOutput contains fully computed balance events with final state
    for balance_event in &output.balance_events {
        log::debug!(target: LOG_TARGET,
            "Balance event: user={} asset={} type={} delta_avail={} delta_frozen={} avail={} frozen={}",
            balance_event.user_id,
            balance_event.asset_id,
            balance_event.event_type,
            balance_event.delta_avail,
            balance_event.delta_frozen,
            balance_event.avail,
            balance_event.frozen
        );

        // Append all balance events - they contain the computed final state
        if let Err(e) = settlement_db.append_balance_event(
            balance_event.user_id,
            balance_event.asset_id,
            balance_event.seq,
            balance_event.delta_avail,
            balance_event.delta_frozen,
            balance_event.avail,
            balance_event.frozen,
            &balance_event.event_type,
            balance_event.ref_id,
        ).await {
            log::error!(target: LOG_TARGET,
                "Failed to append balance event: user={} asset={} type={}: {}",
                balance_event.user_id, balance_event.asset_id, balance_event.event_type, e);
        }
    }

    // 5. Process trades
    // Note: Balance updates were already done via balance_events above
    // So we just need to record the trade, not update balances again
    for trade in &output.trades {
        // Build MatchExecData from TradeOutput for trade recording
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
            // Version and balance fields - not needed for insert_trade
            buyer_quote_version: 0,
            buyer_base_version: 0,
            seller_base_version: 0,
            seller_quote_version: 0,
            buyer_quote_balance_after: 0,
            buyer_base_balance_after: 0,
            seller_base_balance_after: 0,
            seller_quote_balance_after: 0,
        };

        // Just insert the trade record - balances were already updated via balance_events
        if let Err(e) = settlement_db.insert_trade(&match_exec).await {
            log::error!(target: LOG_TARGET, "Failed to insert trade {}: {}", trade.trade_id, e);
        } else {
            log::info!(target: LOG_TARGET, "✅ Recorded trade {} from EngineOutput", trade.trade_id);

            // StarRocks
            let sr_client = starrocks_client.clone();
            let ts = if trade.settled_at > 0 { trade.settled_at as i64 } else { Utc::now().timestamp_millis() };
            let sr_trade = StarRocksTrade::from_match_exec(&match_exec, ts);
            tokio::spawn(async move {
                let _ = sr_client.load_trade(sr_trade).await;
            });

            // CSV
            let _ = csv_writer.serialize(&match_exec);
            let _ = csv_writer.flush();
        }

        // Order history fills
        if let Err(e) = process_trade_fill(order_history_db, trade.buyer_user_id, trade.buy_order_id, trade.quantity, event_id).await {
            log::error!(target: LOG_TARGET, "Order History Fill (Buyer) Failed: {}", e);
        }
        if let Err(e) = process_trade_fill(order_history_db, trade.seller_user_id, trade.sell_order_id, trade.quantity, event_id).await {
            log::error!(target: LOG_TARGET, "Order History Fill (Seller) Failed: {}", e);
        }
    }

    // 6. Process order update from input (for new orders)
    if let InputData::PlaceOrder(ref place_order) = output.input.data {
        if let Some(ref order_update) = output.order_update {
            // Convert EngineOutput::OrderUpdate to ledger::OrderUpdate for order history
            let status = match order_update.status {
                1 => OrderStatus::New,
                2 => OrderStatus::PartiallyFilled,
                3 => OrderStatus::Filled,
                4 => OrderStatus::Cancelled,
                5 => OrderStatus::Rejected,
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

            if let Err(e) = process_order_update(order_history_db, &ledger_order_update, event_id).await {
                log::error!(target: LOG_TARGET, "Order History Update Failed: {}", e);
            }
        }
    }

    // 7. Update chain state
    *last_processed_hash = output.hash;
    *last_output_seq = output.output_seq;

    Ok(())
}
