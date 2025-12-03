use std::collections::HashSet;
use std::path::Path;

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;

use fetcher::ledger::{GlobalLedger, Ledger, LedgerCommand};
use fetcher::models::{BalanceRequest, DepositSource};

struct BalanceProcessor {
    ledger: GlobalLedger,
    processed_requests: HashSet<String>, // Idempotency tracking
}

impl BalanceProcessor {
    fn new(ledger: GlobalLedger) -> Self {
        Self {
            ledger,
            processed_requests: HashSet::new(),
        }
    }

    fn process_balance_request(&mut self, req: BalanceRequest) -> Result<(), anyhow::Error> {
        match req {
            BalanceRequest::Deposit {
                request_id,
                user_id,
                asset_id,
                amount,
                source,
                external_tx_id,
                confirmations,
            } => {
                // 1. Check idempotency
                if self.processed_requests.contains(&request_id) {
                    println!("⚠️  Duplicate deposit request: {}", request_id);
                    return Ok(());
                }

                // 2. Validate deposit
                if !self.validate_deposit(&source, confirmations) {
                    println!("❌ Deposit validation failed: {}", request_id);
                    return Ok(());
                }

                // 3. Apply to ledger
                self.ledger.apply(&LedgerCommand::Deposit {
                    user_id,
                    asset: asset_id,
                    amount,
                })?;

                // 4. Mark as processed
                self.processed_requests.insert(request_id.clone());

                println!(
                    "✅ Deposit processed: {} units of asset {} for user {} (tx: {})",
                    amount, asset_id, user_id, external_tx_id
                );
            }

            BalanceRequest::Withdraw {
                request_id,
                user_id,
                asset_id,
                amount,
                destination,
                external_address,
            } => {
                // 1. Check idempotency
                if self.processed_requests.contains(&request_id) {
                    println!("⚠️  Duplicate withdraw request: {}", request_id);
                    return Ok(());
                }

                // 2. Check balance
                let balance = self.ledger.get_balance(user_id, asset_id);
                if balance < amount {
                    println!(
                        "❌ Insufficient balance for withdrawal: user {} needs {} but has {}",
                        user_id, amount, balance
                    );
                    return Err(anyhow::anyhow!("Insufficient balance"));
                }

                // 3. Lock funds (freeze until external tx confirms)
                self.ledger.apply(&LedgerCommand::Lock {
                    user_id,
                    asset: asset_id,
                    amount,
                })?;

                // 4. Mark as processed (pending external confirmation)
                self.processed_requests.insert(request_id.clone());

                // 5. Log external withdrawal initiation
                println!(
                    "🔒 Withdrawal locked: {} units of asset {} for user {} to {}",
                    amount, asset_id, user_id, external_address
                );
                println!("   Destination: {:?}", destination);
                println!("   ⏳ Awaiting external confirmation...");
            }

            BalanceRequest::WithdrawConfirm {
                request_id,
                external_tx_id,
            } => {
                println!(
                    "✅ Withdrawal confirmed: {} (external tx: {})",
                    request_id, external_tx_id
                );
                println!("   Frozen funds will be spent (implement tracking mechanism)");
                // TODO: Implement withdrawal tracking to spend the correct frozen funds
                // This requires maintaining a map of request_id -> (user_id, asset_id, amount)
            }

            BalanceRequest::WithdrawReject {
                request_id,
                reason,
            } => {
                println!(
                    "❌ Withdrawal rejected: {} - {}",
                    request_id, reason
                );
                println!("   Frozen funds will be unlocked (implement tracking mechanism)");
                // TODO: Implement withdrawal tracking to unlock the correct frozen funds
            }
        }

        Ok(())
    }

    fn validate_deposit(&self, source: &DepositSource, confirmations: u32) -> bool {
        match source {
            DepositSource::Blockchain {
                chain,
                required_confirmations,
            } => {
                if confirmations >= *required_confirmations {
                    println!(
                        "   ✓ Blockchain deposit validated: {} confirmations on {} (required: {})",
                        confirmations, chain, required_confirmations
                    );
                    true
                } else {
                    println!(
                        "   ✗ Insufficient confirmations: {} on {} (required: {})",
                        confirmations, chain, required_confirmations
                    );
                    false
                }
            }
            DepositSource::BankTransfer { reference } => {
                println!("   ✓ Bank transfer validated: {}", reference);
                true
            }
            DepositSource::Internal { from_user_id } => {
                println!("   ✓ Internal transfer validated from user {}", from_user_id);
                true
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");
    let wal_dir = Path::new("balance_processor_wal");
    let snap_dir = Path::new("balance_processor_snapshots");

    // Initialize ledger
    let ledger = GlobalLedger::new(wal_dir, snap_dir).expect("Failed to create ledger");
    let mut processor = BalanceProcessor::new(ledger);

    // Kafka Consumer Setup
    let consumer: StreamConsumer = ClientConfig::new()
        .set("group.id", "balance_processor_group")
        .set("bootstrap.servers", &config.kafka.broker)
        .set("auto.offset.reset", "earliest")
        .set("session.timeout.ms", &config.kafka.session_timeout_ms)
        .set("heartbeat.interval.ms", &config.kafka.heartbeat_interval_ms)
        .create()
        .expect("Consumer creation failed");

    let balance_topic = config
        .kafka
        .topics
        .balance_ops
        .clone()
        .unwrap_or_else(|| "balance_ops".to_string());

    consumer
        .subscribe(&[&balance_topic])
        .expect("Can't subscribe to balance_ops topic");

    println!("--------------------------------------------------");
    println!("Balance Processor Started");
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Balance Topic:     {}", balance_topic);
    println!("  Consumer Group:    balance_processor_group");
    println!("  WAL Directory:     {:?}", wal_dir);
    println!("  Snapshot Dir:      {:?}", snap_dir);
    println!("--------------------------------------------------");

    let mut total_processed = 0;
    let mut last_report = std::time::Instant::now();

    loop {
        match consumer.recv().await {
            Ok(m) => {
                if let Some(payload) = m.payload_view::<str>() {
                    match payload {
                        Ok(text) => {
                            match serde_json::from_str::<BalanceRequest>(text) {
                                Ok(req) => {
                                    println!("\n📨 Received: {:?}", req);
                                    
                                    if let Err(e) = processor.process_balance_request(req) {
                                        eprintln!("❌ Failed to process request: {}", e);
                                    }
                                    
                                    total_processed += 1;
                                }
                                Err(e) => {
                                    eprintln!("Failed to parse balance request: {}", e);
                                    eprintln!("Payload: {}", text);
                                }
                            }
                        }
                        Err(e) => eprintln!("Error reading payload: {}", e),
                    }
                }

                // Periodic reporting
                if last_report.elapsed() >= std::time::Duration::from_secs(10) {
                    println!(
                        "\n📊 Stats: {} requests processed, {} unique requests tracked",
                        total_processed,
                        processor.processed_requests.len()
                    );
                    last_report = std::time::Instant::now();
                }
            }
            Err(e) => eprintln!("Kafka error: {}", e),
        }
    }
}
