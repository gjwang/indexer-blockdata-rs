//! Settlement Service - Process EngineOutput messages from the Matching Engine
//!
//! Architecture:
//! 1. Receive EngineOutput via ZMQ PULL socket
//! 2. Verify chain integrity (hash chain + sequence)
//! 3. Batch write to Source of Truth (ScyllaDB engine_output_log)
//! 4. Dispatch to derived writers via bounded channels (back-pressure enabled)
//!
//! Derived Writers (async, ordered, replay-safe):
//! - Balance Events -> ScyllaDB
//! - Trades -> ScyllaDB
//! - Order Updates -> ScyllaDB
//! - Trades -> StarRocks (analytics)

use chrono::Utc;
use fetcher::configure;
use fetcher::db::{OrderHistoryDb, SettlementDb};
use fetcher::engine_output::{EngineOutput, InputData, GENESIS_HASH};
use fetcher::ledger::{OrderStatus, OrderUpdate};
use fetcher::logger::setup_logger;
use fetcher::starrocks_client::{StarRocksClient, StarRocksTrade};
use futures_util::future::join_all;
use std::sync::Arc;
use tokio::sync::mpsc;
use zmq::{Context, PULL};

const LOG_TARGET: &str = "settlement";

// === CONFIGURATION ===
const MAX_BATCH_SIZE: usize = 500;
const MAX_DRAIN_BATCH: usize = 200; // Max items to drain from channel per write
const NUM_BALANCE_WRITERS: usize = 4; // Parallel balance writers for higher throughput
const DERIVED_WRITER_BUFFER: usize = 10_000;

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() {
    // --- Configuration ---
    let config = configure::load_service_config("settlement_config")
        .expect("Failed to load settlement configuration");

    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    let zmq_config = config.zeromq.expect("ZMQ config missing");
    let scylla_config = config.scylladb.expect("ScyllaDB config missing");

    // --- Database Connections ---
    let settlement_db = connect_settlement_db(&scylla_config).await;
    let order_history_db = connect_order_history_db(&scylla_config).await;
    let starrocks_client = Arc::new(StarRocksClient::new());

    // --- ZMQ Setup ---
    let subscriber = setup_zmq(&zmq_config);

    // --- File Writers ---
    let data_dir = configure::prepare_data_dir(&config.data_dir).unwrap_or_else(|e| {
        log::error!(target: LOG_TARGET, "Data dir error: {}", e);
        "./data".to_string()
    });
    let backup_csv_path =
        std::path::PathBuf::from(&data_dir).join(if config.backup_csv_file.is_empty() {
            "settled_trades.csv"
        } else {
            &config.backup_csv_file
        });
    let mut csv_writer = create_csv_writer(&backup_csv_path);

    // --- Load Chain State (Crash Recovery) ---
    let (mut last_seq, mut last_hash) = load_chain_state(&settlement_db).await;

    // --- Spawn Derived Writers ---
    let (balance_tx, trades_tx, order_tx, starrocks_tx) =
        spawn_derived_writers(settlement_db.clone(), order_history_db.clone(), starrocks_client);

    log::info!(target: LOG_TARGET, "✅ Settlement Service Ready");

    // --- Main Processing Loop ---
    run_processing_loop(
        subscriber,
        settlement_db,
        &mut csv_writer,
        &mut last_seq,
        &mut last_hash,
        balance_tx,
        trades_tx,
        order_tx,
        starrocks_tx,
    )
    .await;
}

// ============================================================================
// INITIALIZATION HELPERS
// ============================================================================

async fn connect_settlement_db(config: &fetcher::configure::ScyllaDbConfig) -> Arc<SettlementDb> {
    log::info!(target: LOG_TARGET, "Connecting to ScyllaDB (Settlement)...");
    match SettlementDb::connect(config).await {
        Ok(db) => {
            log::info!(target: LOG_TARGET, "✅ Connected to ScyllaDB (Settlement)");
            Arc::new(db)
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to Settlement DB: {}", e);
            std::process::exit(1);
        }
    }
}

async fn connect_order_history_db(
    config: &fetcher::configure::ScyllaDbConfig,
) -> Arc<OrderHistoryDb> {
    log::info!(target: LOG_TARGET, "Connecting to ScyllaDB (OrderHistory)...");
    match OrderHistoryDb::connect(config).await {
        Ok(db) => {
            log::info!(target: LOG_TARGET, "✅ Connected to ScyllaDB (OrderHistory)");
            Arc::new(db)
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to OrderHistory DB: {}", e);
            std::process::exit(1);
        }
    }
}

fn setup_zmq(config: &fetcher::configure::ZmqConfig) -> zmq::Socket {
    let context = Context::new();

    // PULL socket for data
    let subscriber = context.socket(PULL).expect("Failed to create PULL socket");
    subscriber.set_rcvhwm(1_000_000).expect("Failed to set RCVHWM");
    let endpoint = format!("tcp://*:{}", config.settlement_port);
    subscriber.bind(&endpoint).expect("Failed to bind to settlement port");
    log::info!(target: LOG_TARGET, "📡 Listening on tcp://localhost:{}", config.settlement_port);

    // REP socket for sync handshake with Matching Engine (in separate thread)
    let sync_port = config.settlement_port + 2;
    let sync_context = Context::new();
    std::thread::spawn(move || {
        let sync_socket = sync_context.socket(zmq::REP).expect("Failed to create REP socket");
        sync_socket.bind(&format!("tcp://*:{}", sync_port)).expect("Failed to bind sync socket");
        log::info!(target: "settlement", "🔄 Sync socket on port {}", sync_port);
        log::info!(target: "settlement", "⏳ Waiting for ME sync...");
        if let Ok(msg) = sync_socket.recv_bytes(0) {
            if msg == b"PING" {
                let _ = sync_socket.send(&b"READY"[..], 0);
                log::info!(target: "settlement", "✅ Sent READY to ME");
            }
        }
    });

    subscriber
}

fn create_csv_writer(path: &std::path::Path) -> csv::Writer<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(path)
        .expect("Failed to open backup CSV file");
    log::info!(target: LOG_TARGET, "📂 CSV backup: {}", path.display());
    csv::WriterBuilder::new().has_headers(false).from_writer(file)
}

async fn load_chain_state(db: &SettlementDb) -> (u64, u64) {
    match db.get_chain_state().await {
        Ok((seq, hash)) => {
            if seq > 0 {
                log::info!(target: LOG_TARGET, "🔄 Recovered: seq={} hash={}", seq, hash);
            }
            (seq, hash)
        }
        Err(e) => {
            log::warn!(target: LOG_TARGET, "⚠️ Chain state load failed: {}", e);
            (0, GENESIS_HASH)
        }
    }
}

// ============================================================================
// DERIVED WRITERS
// ============================================================================

use fetcher::engine_output::{BalanceEvent, TradeOutput};

type BalanceTx = Vec<mpsc::Sender<Vec<BalanceEvent>>>;
type TradesTx = mpsc::Sender<(Vec<TradeOutput>, u64)>;
type OrderTx = mpsc::Sender<Vec<EngineOutput>>;
type StarRocksTx = mpsc::Sender<Vec<StarRocksTrade>>;

fn spawn_derived_writers(
    settlement_db: Arc<SettlementDb>,
    order_history_db: Arc<OrderHistoryDb>,
    starrocks_client: Arc<StarRocksClient>,
) -> (BalanceTx, TradesTx, OrderTx, StarRocksTx) {
    // Parallel Balance Events Writers - DRAIN + BATCH (sharded by user_id)
    let mut balance_channels = Vec::with_capacity(NUM_BALANCE_WRITERS);

    for shard_id in 0..NUM_BALANCE_WRITERS {
        let (balance_tx, mut balance_rx) = mpsc::channel::<Vec<BalanceEvent>>(DERIVED_WRITER_BUFFER);
        balance_channels.push(balance_tx);

        let db = settlement_db.clone();
        tokio::spawn(async move {
            while let Some(mut all_events) = balance_rx.recv().await {
                // Drain channel: collect pending balance events (up to MAX_DRAIN_BATCH)
                let mut drain_count = 1;
                while drain_count < MAX_DRAIN_BATCH {
                    match balance_rx.try_recv() {
                        Ok(more_events) => {
                            all_events.extend(more_events);
                            drain_count += 1;
                        }
                        Err(_) => break,
                    }
                }
                // Single batch write for ALL drained events
                if let Err(e) = db.append_balance_events_batch(&all_events).await {
                    log::error!(target: LOG_TARGET, "Balance writer shard {} failed: {}", shard_id, e);
                }
            }
        });
    }

    // Trades Writer - DRAIN + BATCH
    let (trades_tx, mut trades_rx) =
        mpsc::channel::<(Vec<TradeOutput>, u64)>(DERIVED_WRITER_BUFFER);
    let db = settlement_db.clone();
    tokio::spawn(async move {
        while let Some((mut all_trades, mut last_seq)) = trades_rx.recv().await {
            // Drain channel: collect pending trades (up to MAX_DRAIN_BATCH)
            let mut drain_count = 1;
            while drain_count < MAX_DRAIN_BATCH {
                match trades_rx.try_recv() {
                    Ok((more_trades, seq)) => {
                        all_trades.extend(more_trades);
                        last_seq = seq; // Use the sequence of the last received batch
                        drain_count += 1;
                    }
                    Err(_) => break,
                }
            }
            if let Err(e) = db.insert_trades_batch(&all_trades, last_seq).await {
                log::error!(target: LOG_TARGET, "Trades write failed: {}", e);
            }
        }
    });

    // Order History Writer - BATCH + PARALLEL
    let (order_tx, mut order_rx) = mpsc::channel::<Vec<EngineOutput>>(DERIVED_WRITER_BUFFER);
    let db = order_history_db;
    tokio::spawn(async move {
        while let Some(outputs) = order_rx.recv().await {
            // Process all order updates in parallel
            let futures: Vec<_> = outputs
                .iter()
                .map(|output| process_engine_order_update_fast(&db, output))
                .collect();
            join_all(futures).await;
        }
    });

    // StarRocks Writer - PARALLEL HTTP calls
    let (starrocks_tx, mut starrocks_rx) =
        mpsc::channel::<Vec<StarRocksTrade>>(DERIVED_WRITER_BUFFER);
    let sr_client = starrocks_client;
    tokio::spawn(async move {
        while let Some(trades) = starrocks_rx.recv().await {
            // Process all trades in parallel
            let futures: Vec<_> = trades
                .into_iter()
                .map(|trade| {
                    let client = sr_client.clone();
                    async move {
                        if let Err(e) = client.load_trade(trade).await {
                            log::error!(target: "settlement", "StarRocks write failed: {}", e);
                        }
                    }
                })
                .collect();
            join_all(futures).await;
        }
    });

    log::info!(target: LOG_TARGET, "✅ Derived writers spawned: {} balance writers (sharded), buffer={}", NUM_BALANCE_WRITERS, DERIVED_WRITER_BUFFER);
    (balance_channels, trades_tx, order_tx, starrocks_tx)
}

// ============================================================================
// MAIN PROCESSING LOOP
// ============================================================================

async fn run_processing_loop(
    subscriber: zmq::Socket,
    settlement_db: Arc<SettlementDb>,
    csv_writer: &mut csv::Writer<std::fs::File>,
    last_seq: &mut u64,
    last_hash: &mut u64,
    balance_tx: BalanceTx,
    trades_tx: TradesTx,
    order_tx: OrderTx,
    starrocks_tx: StarRocksTx,
) {
    let mut total_msgs: u64 = 0;
    let mut total_trades: u64 = 0;
    let mut total_processing_us: u64 = 0;

    loop {
        // --- Step 1: Receive Batch ---
        let batch = receive_batch(&subscriber, MAX_BATCH_SIZE);
        if batch.is_empty() {
            continue;
        }

        let t_start = std::time::Instant::now();

        // --- Step 2: Verify Chain Integrity ---
        let verified = verify_batch(batch, last_seq, last_hash);
        if verified.is_empty() {
            continue;
        }

        // --- Step 3: Write to Source of Truth (CRITICAL) ---
        if let Err(e) = settlement_db.write_engine_outputs_batch(&verified).await {
            log::error!(target: LOG_TARGET, "❌ SOT write FAILED: {}", e);
            // TODO: Implement retry or halt strategy
            continue;
        }
        let t_sot = t_start.elapsed();

        // --- Step 4: Dispatch to Derived Writers ---
        let t_dispatch = std::time::Instant::now();
        let batch_trades = dispatch_to_writers(
            &verified,
            &balance_tx,
            &trades_tx,
            &order_tx,
            &starrocks_tx,
            csv_writer,
        )
        .await;
        let d_dispatch = t_dispatch.elapsed();

        // --- Metrics ---
        let batch_size = verified.len();
        total_msgs += batch_size as u64;
        total_trades += batch_trades as u64;
        total_processing_us += t_start.elapsed().as_micros() as u64;

        let batch_ops =
            (batch_size as f64 * 1_000_000.0) / t_start.elapsed().as_micros().max(1) as f64;
        let total_ops = (total_msgs as f64 * 1_000_000.0) / total_processing_us.max(1) as f64;

        // Buffer capacity monitoring (remaining = available slots, lower = more backed up)
        let bal_cap = balance_tx.capacity();
        let trd_cap = trades_tx.capacity();
        let ord_cap = order_tx.capacity();
        let sr_cap = starrocks_tx.capacity();

        log::info!(target: LOG_TARGET,
            "[PROGRESS] seq={} msgs={} trades={} | batch: n={} {:.0}op/s sot={:?} dispatch={:?} | total: {:.0}msg/s | buf: bal={} trd={} ord={} sr={}",
            last_seq, total_msgs, total_trades,
            batch_size, batch_ops, t_sot, d_dispatch,
            total_ops,
            bal_cap, trd_cap, ord_cap, sr_cap
        );
    }
}

// ============================================================================
// BATCH PROCESSING HELPERS
// ============================================================================

/// Receive up to `max_size` messages from ZMQ (first blocking, rest non-blocking)
fn receive_batch(subscriber: &zmq::Socket, max_size: usize) -> Vec<EngineOutput> {
    let mut batch = Vec::with_capacity(max_size);

    // First message: blocking
    match subscriber.recv_bytes(0) {
        Ok(data) => {
            if let Ok(output) = serde_json::from_slice::<EngineOutput>(&data) {
                batch.push(output);
            }
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "ZMQ recv error: {}", e);
            return batch;
        }
    }

    // Drain more messages non-blocking
    while batch.len() < max_size {
        match subscriber.recv_bytes(zmq::DONTWAIT) {
            Ok(data) => {
                if let Ok(output) = serde_json::from_slice::<EngineOutput>(&data) {
                    batch.push(output);
                }
            }
            Err(_) => break,
        }
    }

    batch
}

/// Verify batch integrity and update chain state
fn verify_batch(
    batch: Vec<EngineOutput>,
    last_seq: &mut u64,
    last_hash: &mut u64,
) -> Vec<EngineOutput> {
    let mut verified = Vec::with_capacity(batch.len());

    for output in batch {
        let seq = output.output_seq;
        let expected = *last_seq + 1;

        // Skip if already processed (idempotency)
        if seq < expected {
            continue;
        }

        // Sequence must match (no reordering needed now)
        if seq != expected {
            log::error!(target: LOG_TARGET, "Sequence gap: expected {} got {}", expected, seq);
            continue;
        }

        // Hash chain verification
        if !output.verify_prev_hash(*last_hash) {
            log::error!(target: LOG_TARGET, "Hash chain broken at seq={}", seq);
            continue;
        }

        // Output integrity
        if !output.verify() {
            log::error!(target: LOG_TARGET, "Output hash invalid at seq={}", seq);
            continue;
        }

        // Update chain state
        *last_hash = output.hash;
        *last_seq = seq;
        verified.push(output);
    }

    verified
}

/// Dispatch verified outputs to all derived writers
async fn dispatch_to_writers(
    outputs: &[EngineOutput],
    balance_tx: &BalanceTx,
    trades_tx: &TradesTx,
    order_tx: &OrderTx,
    starrocks_tx: &StarRocksTx,
    csv_writer: &mut csv::Writer<std::fs::File>,
) -> usize {
    let mut trade_count = 0;

    for output in outputs {
        // Partition balance events by user_id to sharded writers
        if !output.balance_events.is_empty() {
            // Group events by shard (user_id % NUM_BALANCE_WRITERS)
            use std::collections::HashMap;
            let mut sharded_events: HashMap<usize, Vec<BalanceEvent>> = HashMap::new();

            for event in &output.balance_events {
                let shard = (event.user_id % NUM_BALANCE_WRITERS as u64) as usize;
                sharded_events.entry(shard).or_insert_with(Vec::new).push(event.clone());
            }

            // Send to appropriate shards
            for (shard, events) in sharded_events {
                let _ = balance_tx[shard].send(events).await;
            }
        }

        // Send trades per-output (writer will drain)
        if !output.trades.is_empty() {
            trade_count += output.trades.len();
            let _ = trades_tx.send((output.trades.clone(), output.output_seq)).await;
        }
    }

    // Order Updates - send entire batch at once
    let _ = order_tx.send(outputs.to_vec()).await;

    // StarRocks TEMPORARILY DISABLED
    let _ = starrocks_tx;

    let _ = csv_writer.flush();
    trade_count
}

// ============================================================================
// ORDER HISTORY PROCESSING - PARALLEL VERSION
// ============================================================================

/// Process order update with PARALLEL DB writes (3-5x faster)
async fn process_order_update_parallel(
    db: &OrderHistoryDb,
    order_update: &OrderUpdate,
    event_id: u64,
) {
    match order_update.status {
        OrderStatus::New => {
            // 5 operations in parallel
            let (r1, r2, r3, r4, r5) = tokio::join!(
                db.upsert_active_order(order_update),
                db.insert_order_history(order_update),
                db.insert_order_update_stream(order_update, event_id),
                db.init_user_statistics(order_update.user_id, order_update.timestamp),
                db.update_order_statistics(
                    order_update.user_id,
                    &order_update.status,
                    order_update.timestamp
                )
            );
            if r1.is_err() || r2.is_err() || r3.is_err() || r4.is_err() || r5.is_err() {
                log::warn!(target: LOG_TARGET, "Order update partial failure for order {}", order_update.order_id);
            }
        }
        OrderStatus::PartiallyFilled => {
            // 3 operations in parallel
            let (r1, r2, r3) = tokio::join!(
                db.upsert_active_order(order_update),
                db.insert_order_history(order_update),
                db.insert_order_update_stream(order_update, event_id)
            );
            if r1.is_err() || r2.is_err() || r3.is_err() {
                log::warn!(target: LOG_TARGET, "Order update partial failure for order {}", order_update.order_id);
            }
        }
        OrderStatus::Filled | OrderStatus::Cancelled | OrderStatus::Expired => {
            // 4 operations in parallel
            let (r1, r2, r3, r4) = tokio::join!(
                db.delete_active_order(order_update.user_id, order_update.order_id),
                db.insert_order_history(order_update),
                db.insert_order_update_stream(order_update, event_id),
                db.update_order_statistics(
                    order_update.user_id,
                    &order_update.status,
                    order_update.timestamp
                )
            );
            if r1.is_err() || r2.is_err() || r3.is_err() || r4.is_err() {
                log::warn!(target: LOG_TARGET, "Order update partial failure for order {}", order_update.order_id);
            }
        }
        OrderStatus::Rejected => {
            // 3 operations in parallel
            let (r1, r2, r3) = tokio::join!(
                db.insert_order_history(order_update),
                db.insert_order_update_stream(order_update, event_id),
                db.update_order_statistics(
                    order_update.user_id,
                    &order_update.status,
                    order_update.timestamp
                )
            );
            if r1.is_err() || r2.is_err() || r3.is_err() {
                log::warn!(target: LOG_TARGET, "Order update partial failure for order {}", order_update.order_id);
            }
        }
    }
}

/// Fast parallel order update processing
async fn process_engine_order_update_fast(db: &OrderHistoryDb, output: &EngineOutput) {
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
                avg_fill_price: if order_update.avg_price > 0 {
                    Some(order_update.avg_price)
                } else {
                    None
                },
                rejection_reason: None,
                timestamp: order_update.updated_at,
                match_id: None,
            };

            process_order_update_parallel(db, &ledger_order_update, 0).await;
        }
    }
}
