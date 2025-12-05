use fetcher::configure;
use fetcher::db::OrderHistoryDb;
use fetcher::ledger::{LedgerCommand, OrderStatus, OrderUpdate};
use fetcher::logger::setup_logger;
use zmq::{Context, PULL};

const LOG_TARGET: &str = "order_history";

#[tokio::main]
async fn main() {
    // Load service-specific configuration
    let config = configure::load_service_config("order_history_config")
        .expect("Failed to load order history configuration");

    // Setup logger
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    log::info!(target: LOG_TARGET, "🚀 Order History Service starting...");

    // Get configurations
    let zmq_config = config.zeromq.expect("ZMQ config missing");
    let scylla_config = config.scylladb.expect("ScyllaDB config missing");

    // Initialize ScyllaDB connection
    log::info!(target: LOG_TARGET, "Connecting to ScyllaDB...");
    let order_history_db = match OrderHistoryDb::connect(&scylla_config).await {
        Ok(db) => {
            log::info!(target: LOG_TARGET, "✅ Connected to ScyllaDB");
            std::sync::Arc::new(db)
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ Failed to connect to ScyllaDB: {}", e);
            log::error!(target: LOG_TARGET, "   Make sure ScyllaDB is running: docker-compose up -d scylla");
            log::error!(target: LOG_TARGET, "   And schema is initialized: ./scripts/init_order_history_schema.sh");
            return;
        }
    };

    // Health check
    match order_history_db.health_check().await {
        Ok(true) => log::info!(target: LOG_TARGET, "✅ ScyllaDB health check passed"),
        Ok(false) => {
            log::error!(target: LOG_TARGET, "❌ ScyllaDB health check failed");
            return;
        }
        Err(e) => {
            log::error!(target: LOG_TARGET, "❌ ScyllaDB health check error: {}", e);
            return;
        }
    }

    // Spawn background health check task
    let db_clone = order_history_db.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            match db_clone.health_check().await {
                Ok(true) => log::debug!(target: LOG_TARGET, "[HEALTH] ScyllaDB connection active"),
                Ok(false) => log::error!(target: LOG_TARGET, "[HEALTH] ScyllaDB connection lost!"),
                Err(e) => log::error!(target: LOG_TARGET, "[HEALTH] ScyllaDB health check error: {}", e),
            }
        }
    });

    // Setup ZMQ PULL socket
    let context = Context::new();
    let subscriber = context.socket(PULL).expect("Failed to create PULL socket");
    subscriber.set_rcvhwm(1_000_000).expect("Failed to set RCVHWM");

    let endpoint = format!("tcp://localhost:{}", zmq_config.settlement_port);
    subscriber.connect(&endpoint).expect("Failed to connect to settlement port");

    log::info!(target: LOG_TARGET, "=== Order History Service Boot Parameters ===");
    log::info!(target: LOG_TARGET, "  ZMQ Endpoint:     {}", endpoint);
    log::info!(target: LOG_TARGET, "  ScyllaDB Hosts:   {:?}", scylla_config.hosts);
    log::info!(target: LOG_TARGET, "  ScyllaDB Keyspace: {}", scylla_config.keyspace);
    log::info!(target: LOG_TARGET, "  Log File:         {}", config.log_file);
    log::info!(target: LOG_TARGET, "  Log Level:        {}", config.log_level);
    log::info!(target: LOG_TARGET, "=============================================");

    log::info!(target: LOG_TARGET, "✅ Order History Service started");
    log::info!(target: LOG_TARGET, "📡 Listening on {}", endpoint);
    log::info!(target: LOG_TARGET, "⏳ Waiting for OrderUpdate events...");

    let mut total_events = 0u64;
    let mut total_persisted = 0u64;
    let mut total_errors = 0u64;
    let mut events_by_status = std::collections::HashMap::new();

    loop {
        // Receive data directly (PUSH/PULL doesn't use topics)
        let data = match subscriber.recv_bytes(0) {
            Ok(d) => {
                log::info!(target: LOG_TARGET, "📨 Received {} bytes from ZMQ", d.len());
                d
            },
            Err(e) => {
                log::error!(target: LOG_TARGET, "ZMQ recv bytes error: {}", e);
                continue;
            }
        };

        // Deserialize as LedgerCommand
        match serde_json::from_slice::<LedgerCommand>(&data) {
            Ok(LedgerCommand::OrderUpdate(order_update)) => {
                total_events += 1;
                *events_by_status.entry(order_update.status.clone()).or_insert(0) += 1;

                log::info!(
                    target: LOG_TARGET,
                    "📋 OrderUpdate #{} | Order #{} | User {} | {} | {} | Price: {} | Qty: {} | Filled: {}",
                    total_events,
                    order_update.order_id,
                    order_update.user_id,
                    order_update.symbol,
                    format_status(&order_update.status),
                    order_update.price,
                    order_update.qty,
                    order_update.filled_qty
                );

                // Process the order update
                match process_order_update(&order_history_db, &order_update, total_events).await {
                    Ok(()) => {
                        total_persisted += 1;
                        log::debug!(target: LOG_TARGET, "  ✅ Persisted to ScyllaDB");
                    }
                    Err(e) => {
                        total_errors += 1;
                        log::error!(
                            target: LOG_TARGET,
                            "❌ Failed to persist OrderUpdate #{}: {}",
                            order_update.order_id,
                            e
                        );
                    }
                }

                // Log statistics every 100 events
                if total_events % 100 == 0 {
                    log::info!(target: LOG_TARGET, "📊 Statistics:");
                    log::info!(target: LOG_TARGET, "   Total Events: {}", total_events);
                    log::info!(target: LOG_TARGET, "   Persisted: {}", total_persisted);
                    log::info!(target: LOG_TARGET, "   Errors: {}", total_errors);
                    for (status, count) in &events_by_status {
                        log::info!(target: LOG_TARGET, "   {:?}: {}", status, count);
                    }
                }
            }
            Ok(LedgerCommand::MatchExecBatch(trades)) => {
                log::info!(target: LOG_TARGET, "Received MatchExecBatch with {} trades", trades.len());
                for trade in trades {
                     // Process buyer
                     if let Err(e) = process_trade_fill(&order_history_db, trade.buyer_user_id, trade.buy_order_id, trade.quantity, trade.match_seq).await {
                         log::error!(target: LOG_TARGET, "Failed to process buy fill: {}", e);
                     }
                     // Process seller
                     if let Err(e) = process_trade_fill(&order_history_db, trade.seller_user_id, trade.sell_order_id, trade.quantity, trade.match_seq).await {
                         log::error!(target: LOG_TARGET, "Failed to process sell fill: {}", e);
                     }
                }
            }
            Ok(LedgerCommand::MatchExec(trade)) => {
                 log::info!(target: LOG_TARGET, "Received Single MatchExec");
                 // Process buyer
                 if let Err(e) = process_trade_fill(&order_history_db, trade.buyer_user_id, trade.buy_order_id, trade.quantity, trade.match_seq).await {
                     log::error!(target: LOG_TARGET, "Failed to process buy fill: {}", e);
                 }
                 // Process seller
                 if let Err(e) = process_trade_fill(&order_history_db, trade.seller_user_id, trade.sell_order_id, trade.quantity, trade.match_seq).await {
                     log::error!(target: LOG_TARGET, "Failed to process sell fill: {}", e);
                 }
            }
            Ok(_other_command) => {
                log::debug!(target: LOG_TARGET, "Received other LedgerCommand (ignored)");
            }
            Err(e) => {
                log::error!(target: LOG_TARGET, "Failed to deserialize LedgerCommand: {}", e);
            }
        }
    }
}

/// Process an OrderUpdate event and persist to ScyllaDB
async fn process_order_update(
    db: &OrderHistoryDb,
    order_update: &OrderUpdate,
    event_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    match order_update.status {
        OrderStatus::New => {
            // 1. Insert into active_orders
            db.upsert_active_order(order_update).await?;

            // 2. Insert into order_history
            db.insert_order_history(order_update).await?;

            // 3. Insert into order_updates_stream
            db.insert_order_update_stream(order_update, event_id).await?;

            // 4. Initialize user statistics (if first order)
            db.init_user_statistics(order_update.user_id, order_update.timestamp).await?;

            // 5. Update statistics
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;

            log::debug!(target: LOG_TARGET, "  → Inserted into active_orders + history");
        }
        OrderStatus::PartiallyFilled => {
            // 1. Update active_orders
            db.upsert_active_order(order_update).await?;

            // 2. Insert into order_history
            db.insert_order_history(order_update).await?;

            // 3. Insert into order_updates_stream
            db.insert_order_update_stream(order_update, event_id).await?;

            log::debug!(target: LOG_TARGET, "  → Updated active_orders");
        }
        OrderStatus::Filled => {
            // 1. Delete from active_orders
            db.delete_active_order(order_update.user_id, order_update.order_id).await?;

            // 2. Insert into order_history
            db.insert_order_history(order_update).await?;

            // 3. Insert into order_updates_stream
            db.insert_order_update_stream(order_update, event_id).await?;

            // 4. Update statistics
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;

            log::debug!(target: LOG_TARGET, "  → Deleted from active_orders (filled)");
        }
        OrderStatus::Cancelled => {
            // 1. Delete from active_orders
            db.delete_active_order(order_update.user_id, order_update.order_id).await?;

            // 2. Insert into order_history
            db.insert_order_history(order_update).await?;

            // 3. Insert into order_updates_stream
            db.insert_order_update_stream(order_update, event_id).await?;

            // 4. Update statistics
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;

            log::debug!(target: LOG_TARGET, "  → Deleted from active_orders (cancelled)");
        }
        OrderStatus::Rejected => {
            // 1. Insert into order_history only (no active_orders entry)
            db.insert_order_history(order_update).await?;

            // 2. Insert into order_updates_stream
            db.insert_order_update_stream(order_update, event_id).await?;

            // 3. Update statistics
            db.update_order_statistics(order_update.user_id, &order_update.status, order_update.timestamp).await?;

            log::debug!(target: LOG_TARGET, "  → Inserted into history only (rejected)");
        }
        OrderStatus::Expired => {
            // 1. Delete from active_orders
            db.delete_active_order(order_update.user_id, order_update.order_id).await?;

            // 2. Insert into order_history
            db.insert_order_history(order_update).await?;

            // 3. Insert into order_updates_stream
            db.insert_order_update_stream(order_update, event_id).await?;

            log::debug!(target: LOG_TARGET, "  → Deleted from active_orders (expired)");
        }
    }

    Ok(())
}

/// Format order status for display
fn format_status(status: &OrderStatus) -> String {
    match status {
        OrderStatus::New => "🆕 NEW".to_string(),
        OrderStatus::PartiallyFilled => "⏳ PARTIAL".to_string(),
        OrderStatus::Filled => "✅ FILLED".to_string(),
        OrderStatus::Cancelled => "❌ CANCELLED".to_string(),
        OrderStatus::Rejected => "🚫 REJECTED".to_string(),
        OrderStatus::Expired => "⏰ EXPIRED".to_string(),
    }
}

/// Process a trade fill for a specific order
async fn process_trade_fill(
    db: &OrderHistoryDb,
    user_id: u64,
    order_id: u64,
    match_qty: u64,
    event_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Fetch current order state
    if let Some(mut order) = db.get_active_order(user_id, order_id).await? {
        // 2. Update state
        order.filled_qty += match_qty;

        if order.filled_qty >= order.qty {
            order.status = OrderStatus::Filled;
        } else {
            order.status = OrderStatus::PartiallyFilled;
        }

        // 3. Persist using existing logic
        process_order_update(db, &order, event_id).await?;

        log::info!(
            target: LOG_TARGET,
            "✨ Processed Fill: Order #{} | User {} | +{} (Total: {}/{}) | Status: {:?}",
            order_id, user_id, match_qty, order.filled_qty, order.qty, order.status
        );
    } else {
        // Order not found in active_orders.
        log::warn!(target: LOG_TARGET, "⚠️ Trade fill for unknown active order #{} (User {})", order_id, user_id);
    }
    Ok(())
}
