use std::sync::Arc;
use scylla::SessionBuilder;
use tigerbeetle_unofficial::Client;
use fetcher::db::InternalTransferDb;
use fetcher::api::internal_transfer_settlement::InternalTransferSettlement;
use fetcher::logging::setup_async_file_logging;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup logging
    let _guard = setup_async_file_logging("internal_transfer_settlement", "logs");
    log::info!("🚀 Internal Transfer Settlement Service Starting...");

    let kafka_brokers = std::env::var("KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".to_string());
    let db_url = std::env::var("SCYLLA_URL").unwrap_or_else(|_| "localhost:9042".to_string());
    let tb_addresses = std::env::var("TB_ADDRESSES").unwrap_or_else(|_| "3000".to_string());

    // Connect to ScyllaDB
    log::info!("Connecting to ScyllaDB at {}", db_url);
    let session = SessionBuilder::new()
        .known_node(db_url)
        .build()
        .await
        .map_err(|e| format!("ScyllaDB connection failed: {}", e))?;
    let db = Arc::new(InternalTransferDb::new(Arc::new(session)));

    // Connect to TigerBeetle
    let addr = if tb_addresses.contains(":") { tb_addresses } else { format!("127.0.0.1:{}", tb_addresses) };
    log::info!("Connecting to TigerBeetle at {}", addr);
    let tb_client = Arc::new(Client::new(0, addr).map_err(|e| format!("TB connect failed: {}", e))?);

    // Initialize Settlement Logic
    let settlement = Arc::new(InternalTransferSettlement::new(db, tb_client));

    // Spawn Background Scanner (Safety Net)
    let scanner = settlement.clone();
    tokio::spawn(async move {
        scanner.run_scanner().await;
    });
    log::info!("✅ Background Scanner spawned");

    // Run Kafka Consumer (Main Loop)
    log::info!("📡 Listening for balance events...");
    let group_id = std::env::var("KAFKA_GROUP_ID").unwrap_or_else(|_| "settlement_group".to_string());
    settlement.run_consumer(kafka_brokers, group_id).await;

    Ok(())
}
