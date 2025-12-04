use fetcher::configure;
use fetcher::db::SettlementDb;
use fetcher::logger::setup_logger;
use fetcher::reconciliation::reconcile_csv;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let config = configure::load_service_config("reconciliation_config")
        .unwrap_or_else(|_| configure::load_config().expect("Failed to load config"));

    setup_logger(&config).expect("Failed to setup logger");

    let scylla_config = config.scylladb.expect("ScyllaDB config missing");
    let db = SettlementDb::connect(&scylla_config).await.expect("Failed to connect to DB");

    let csv_path = "settled_trades.csv";

    loop {
        log::info!("Starting scheduled reconciliation...");
        if std::path::Path::new(csv_path).exists() {
            match reconcile_csv(&db, csv_path).await {
                Ok(stats) => {
                    log::info!("Reconciliation finished: {:?}", stats);
                    if stats.mismatch > 0 || stats.missing > 0 {
                        log::error!("Reconciliation FAILED: Inconsistent data detected!");
                    } else {
                        log::info!("Reconciliation PASSED: Data is consistent.");
                    }
                }
                Err(e) => log::error!("Reconciliation error: {}", e),
            }
        } else {
            log::warn!("Source file {} not found, skipping reconciliation.", csv_path);
        }

        sleep(Duration::from_secs(300)).await; // Run every 5 minutes
    }
}
