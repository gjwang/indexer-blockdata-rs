use crate::db::SettlementDb;
use crate::ledger::MatchExecData;
use anyhow::Result;
use std::io::Write;

#[derive(Debug, Default)]
pub struct ReconciliationStats {
    pub total: u64,
    pub missing: u64,
    pub mismatch: u64,
}

pub async fn reconcile_csv(db: &SettlementDb, file_path: &str) -> Result<ReconciliationStats> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false) // settled_trades.csv has no headers
        .from_path(file_path)?;

    let mut stats = ReconciliationStats::default();

    println!("Starting reconciliation against {}...", file_path);
    for result in rdr.deserialize() {
        let csv_trade: MatchExecData = match result {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Failed to parse CSV record: {}", e);
                continue;
            }
        };
        stats.total += 1;

        match db.get_trade_by_id(csv_trade.trade_id).await {
            Ok(Some(db_trade)) => {
                // Compare fields
                if db_trade.price != csv_trade.price
                    || db_trade.quantity != csv_trade.quantity
                    || db_trade.buyer_user_id != csv_trade.buyer_user_id
                {
                    println!(
                        "❌ Mismatch for Trade {}: CSV={:?}, DB={:?}",
                        csv_trade.trade_id, csv_trade, db_trade
                    );
                    stats.mismatch += 1;
                }
            }
            Ok(None) => {
                println!("❌ Missing Trade {}: {:?}", csv_trade.trade_id, csv_trade);
                stats.missing += 1;
            }
            Err(e) => {
                eprintln!("Error querying trade {}: {}", csv_trade.trade_id, e);
            }
        }

        if stats.total % 100 == 0 {
            print!(".");
            std::io::stdout().flush().unwrap();
        }
    }
    println!(); // Newline after dots

    Ok(stats)
}
