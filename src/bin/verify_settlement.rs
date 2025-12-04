use clap::{Parser, Subcommand};
use fetcher::configure;
use fetcher::db::SettlementDb;
use fetcher::logger::setup_logger;

#[derive(Parser)]
#[command(name = "verify_settlement")]
#[command(about = "Verify settlement data in ScyllaDB")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get a trade by ID
    GetTrade {
        #[arg(long)]
        id: u64,
    },
    /// Get trades by sequence range
    GetRange {
        #[arg(long)]
        start: u64,
        #[arg(long)]
        end: u64,
    },
    /// Check database health
    Health,
    /// Reconcile CSV against ScyllaDB
    Reconcile {
        #[arg(long)]
        file: String,
    },
}

#[tokio::main]
async fn main() {
    // Load configuration
    let config = configure::load_service_config("settlement_config")
        .expect("Failed to load settlement configuration");

    // Setup logger
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    let scylla_config = config.scylladb.expect("ScyllaDB config missing");

    println!("Connecting to ScyllaDB at {:?}...", scylla_config.hosts);
    let db = match SettlementDb::connect(&scylla_config).await {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to connect to ScyllaDB: {}", e);
            return;
        }
    };

    let cli = Cli::parse();

    match cli.command {
        Commands::GetTrade { id } => {
            println!("Querying trade ID: {}", id);
            match db.get_trade_by_id(id).await {
                Ok(Some(trade)) => {
                    println!("✅ Found trade:");
                    println!("{:#?}", trade);
                }
                Ok(None) => {
                    println!("❌ Trade not found");
                }
                Err(e) => {
                    eprintln!("❌ Error querying trade: {}", e);
                }
            }
        }
        Commands::GetRange { start, end } => {
            println!("Querying trades from seq {} to {}", start, end);
            match db.get_trades_by_sequence_range(start, end).await {
                Ok(trades) => {
                    println!("✅ Found {} trades", trades.len());
                    for trade in trades {
                        println!(
                            "  Seq {}: TradeID {}, Price {}, Qty {}",
                            trade.output_sequence, trade.trade_id, trade.price, trade.quantity
                        );
                    }
                }
                Err(e) => {
                    eprintln!("❌ Error querying range: {}", e);
                }
            }
        }
        Commands::Health => match db.health_check().await {
            Ok(true) => println!("✅ Database is healthy"),
            Ok(false) => println!("❌ Database is unhealthy"),
            Err(e) => println!("❌ Health check error: {}", e),
        },
        Commands::Reconcile { file } => {
            println!("Reconciling against file: {}", file);
            let mut rdr = csv::ReaderBuilder::new()
                .has_headers(false) // settled_trades.csv has no headers
                .from_path(file)
                .expect("Failed to open CSV");

            let mut total = 0;
            let mut missing = 0;
            let mut mismatch = 0;

            println!("Starting reconciliation...");
            for result in rdr.deserialize() {
                let csv_trade: fetcher::ledger::MatchExecData = match result {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("Failed to parse CSV record: {}", e);
                        continue;
                    }
                };
                total += 1;

                match db.get_trade_by_id(csv_trade.trade_id).await {
                    Ok(Some(db_trade)) => {
                        // Compare fields
                        if db_trade.price != csv_trade.price ||
                           db_trade.quantity != csv_trade.quantity ||
                           db_trade.buyer_user_id != csv_trade.buyer_user_id {
                               println!("❌ Mismatch for Trade {}: CSV={:?}, DB={:?}", csv_trade.trade_id, csv_trade, db_trade);
                               mismatch += 1;
                           }
                    }
                    Ok(None) => {
                        println!("❌ Missing Trade {}: {:?}", csv_trade.trade_id, csv_trade);
                        missing += 1;
                    }
                    Err(e) => {
                        eprintln!("Error querying trade {}: {}", csv_trade.trade_id, e);
                    }
                }

                if total % 100 == 0 {
                    use std::io::Write;
                    print!(".");
                    std::io::stdout().flush().unwrap();
                }
            }
            println!("\n=== Reconciliation Complete ===");
            println!("Total Processed: {}", total);
            println!("Missing: {}", missing);
            println!("Mismatch: {}", mismatch);

            if missing == 0 && mismatch == 0 {
                println!("✅ Data is CONSISTENT");
            } else {
                println!("❌ Data INCONSISTENT");
            }
        }
    }
}
