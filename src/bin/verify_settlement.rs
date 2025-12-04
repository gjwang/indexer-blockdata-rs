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
            match fetcher::reconciliation::reconcile_csv(&db, &file).await {
                Ok(stats) => {
                    println!("\n=== Reconciliation Complete ===");
                    println!("Total Processed: {}", stats.total);
                    println!("Missing: {}", stats.missing);
                    println!("Mismatch: {}", stats.mismatch);

                    if stats.missing == 0 && stats.mismatch == 0 {
                        println!("✅ Data is CONSISTENT");
                    } else {
                        println!("❌ Data INCONSISTENT");
                    }
                }
                Err(e) => eprintln!("Reconciliation failed: {}", e),
            }
        }
    }
}
