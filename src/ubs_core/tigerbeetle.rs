use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tigerbeetle_unofficial::{Client, Transfer};
use tigerbeetle_unofficial::transfer::Flags as TransferFlags;
use tokio::sync::mpsc;

// Ledger constants
pub const TRADING_LEDGER: u32 = 1;

// Special accounts
// 0xFFFF...FFFF = All 1s (max u64)
pub const EXCHANGE_OMNIBUS_ID_PREFIX: u64 = u64::MAX;
pub const HOLDING_ACCOUNT_ID_PREFIX: u64 = u64::MAX - 1;
pub const REVENUE_ACCOUNT_ID_PREFIX: u64 = u64::MAX - 2;

use crate::ubs_core::events::{BalanceEvent, UnlockReason};

pub struct TigerBeetleWorker;

impl TigerBeetleWorker {
    /// Start the background sync task
    pub fn start(cluster_id: u128, addresses: Vec<String>, rx: mpsc::UnboundedReceiver<BalanceEvent>) -> Result<(), String> {
        // Client::new is synchronous in this version
        let client = Client::new(cluster_id, addresses.join(","))
            .map_err(|e| format!("Failed to create TB client: {:?}", e))?;

        // Spawn background worker
        tokio::spawn(async move {
            Self::worker_loop(client, rx).await;
        });

        Ok(())
    }

    /// Main worker loop processing events
    async fn worker_loop(client: Client, mut rx: mpsc::UnboundedReceiver<BalanceEvent>) {
        log::info!("TigerBeetle Shadow Ledger sync started - Initializing System Accounts...");

        // Initialize System Accounts (Idempotent)
        let mut sys_accounts = Vec::new();
        for asset_id in [1, 2] { // BTC, USDT
             sys_accounts.push(tigerbeetle_unofficial::Account::new(tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id), TRADING_LEDGER, 1));
             sys_accounts.push(tigerbeetle_unofficial::Account::new(tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, asset_id), TRADING_LEDGER, 1));
             sys_accounts.push(tigerbeetle_unofficial::Account::new(tb_account_id(REVENUE_ACCOUNT_ID_PREFIX, asset_id), TRADING_LEDGER, 1));
        }

        match client.create_accounts(sys_accounts).await {
            Ok(_) => log::info!("System accounts initialized (or already existed)"),
            Err(e) => log::error!("Failed to init system accounts: {:?}", e),
        }

        log::info!("Starting event loop...");


        while let Some(event) = rx.recv().await {
            let transfers = match event {
                BalanceEvent::Deposited { tx_id, user_id, asset_id, amount } => {
                    vec![Transfer::new(tb_id(1, tx_id)) // Type 1 = External
                        .with_debit_account_id(tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id))
                        .with_credit_account_id(tb_account_id(user_id, asset_id))
                        .with_amount(amount as u128)
                        .with_ledger(TRADING_LEDGER)
                        .with_code(1)]
                },
                BalanceEvent::Withdrawn { tx_id, user_id, asset_id, amount } => {
                    vec![Transfer::new(tb_id(1, tx_id)) // Type 1 = External
                        .with_debit_account_id(tb_account_id(user_id, asset_id))
                        .with_credit_account_id(tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id))
                        .with_amount(amount as u128)
                        .with_ledger(TRADING_LEDGER)
                        .with_code(1)]
                },
                BalanceEvent::FundsLocked { user_id, asset_id, amount, order_id } => {
                    // Create PENDING transfer: User -> Holding
                    vec![Transfer::new(tb_id(2, order_id)) // Type 2 = Order Lock
                        .with_debit_account_id(tb_account_id(user_id, asset_id))
                        .with_credit_account_id(tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, asset_id))
                        .with_amount(amount as u128)
                        .with_ledger(TRADING_LEDGER)
                        .with_code(1)
                        .with_flags(TransferFlags::PENDING)]
                },
                BalanceEvent::FundsUnlocked { order_id, .. } => {
                    // VOID the pending transfer (Assuming order_id was used as lock_id)
                    vec![Transfer::new(generate_transfer_id())
                        .with_pending_id(tb_id(2, order_id)) // Reference Type 2
                        .with_code(1)
                        .with_flags(TransferFlags::VOID_PENDING_TRANSFER)]
                },
                BalanceEvent::TradeSettled {
                    buyer_id, seller_id, base_asset, quote_asset,
                    base_qty, quote_amt, buyer_order_id, seller_order_id,
                    buyer_fee, seller_fee, ..
                } => {
                    // Atomic Settlement Batch: All LINKED except the last one

                    // 1. Post Buyer (release frozen quote)
                    let post_buyer = Transfer::new(generate_transfer_id())
                        .with_pending_id(tb_id(2, buyer_order_id)) // Reference Type 2
                        .with_amount((quote_amt + buyer_fee) as u128)
                        .with_code(1) // Must match Pending Code
                        .with_flags(TransferFlags::POST_PENDING_TRANSFER | TransferFlags::LINKED);

                    // 2. Post Seller (release frozen base)
                    let post_seller = Transfer::new(generate_transfer_id())
                        .with_pending_id(tb_id(2, seller_order_id)) // Reference Type 2
                        .with_amount(base_qty as u128)
                        .with_code(1) // Must match Pending Code
                        .with_flags(TransferFlags::POST_PENDING_TRANSFER | TransferFlags::LINKED);

                    // 3. Base Transfer: Seller -> Buyer (LINKED)
                    // ... (rest same, omitting for brevity of replacement if possible, but replace tool needs context)
                    // I will include the rest to be safe or target carefully.

                    // 3. Base Transfer: Seller -> Buyer (LINKED)
                    // Source is HOLDING because funds were locked there
                    let base_transfer = Transfer::new(generate_transfer_id())
                        .with_debit_account_id(tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, base_asset))
                        .with_credit_account_id(tb_account_id(buyer_id, base_asset))
                        .with_amount(base_qty as u128)
                        .with_ledger(TRADING_LEDGER)
                        .with_code(2)
                        .with_flags(TransferFlags::LINKED);

                    // 4. Quote Transfer: Buyer -> Seller (LINKED)
                    // Source is HOLDING because funds were locked there
                    let quote_transfer = Transfer::new(generate_transfer_id())
                        .with_debit_account_id(tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, quote_asset))
                        .with_credit_account_id(tb_account_id(seller_id, quote_asset))
                        .with_amount(quote_amt as u128)
                        .with_ledger(TRADING_LEDGER)
                        .with_code(2)
                        .with_flags(TransferFlags::LINKED);

                    // 5. Buyer Fee: Buyer -> Revenue (USDT) (LINKED)
                    // Source is HOLDING because Buyer included fee in lock
                    let buyer_fee_tx = Transfer::new(generate_transfer_id())
                         .with_debit_account_id(tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, quote_asset))
                         .with_credit_account_id(tb_account_id(REVENUE_ACCOUNT_ID_PREFIX, quote_asset))
                         .with_amount(buyer_fee as u128)
                         .with_ledger(TRADING_LEDGER)
                         .with_code(3)
                         .with_flags(TransferFlags::LINKED);

                    // 6. Seller Fee: Seller -> Revenue (USDT) (CLOSES CHAIN - NO LINKED)
                    // Seller pays from proceeds (which hit their account in step 4)
                    let seller_fee_tx = Transfer::new(generate_transfer_id())
                         .with_debit_account_id(tb_account_id(seller_id, quote_asset))
                         .with_credit_account_id(tb_account_id(REVENUE_ACCOUNT_ID_PREFIX, quote_asset))
                         .with_amount(seller_fee as u128)
                         .with_ledger(TRADING_LEDGER)
                         .with_code(3);
                         // NO LINKED FLAG

                    vec![post_buyer, post_seller, base_transfer, quote_transfer, buyer_fee_tx, seller_fee_tx]
                },
                BalanceEvent::AccountCreated { user_id } => {
                     // Create TB accounts for default assets (1=BTC, 2=USDT)
                     let mut accounts = Vec::new();
                     for asset_id in [1, 2] {
                         accounts.push(tigerbeetle_unofficial::Account::new(
                             tb_account_id(user_id, asset_id),
                             TRADING_LEDGER,
                             1 // code
                         ));
                     }
                     // We invoke create_accounts separately.
                     // Since worker_loop expects `transfers`, we need to change the loop to handle mixed?
                     // Or just execute here and return empty transfers?
                     if !accounts.is_empty() {
                        match client.create_accounts(accounts.clone()).await {
                            Ok(_) => {
                                for acc in &accounts {
                                    log::info!("✅ Created TB Account: id={}", acc.id());
                                }
                                log::info!("Created accounts for user {}", user_id);
                            },
                            Err(e) => log::error!("Failed to create accounts for {}: {:?}", user_id, e),
                        }
                    }
                    vec![] // Return empty transfers vector to continue loop
                }
            };

            if !transfers.is_empty() {
                // Execute transfers
                match client.create_transfers(transfers).await {
                    Ok(_) => {
                        log::debug!("Shadow Sync OK");
                    },
                    Err(e) => {
                         log::error!("CRITICAL: Shadow Ledger mismatch! TB rejected transfers: {:?}", e);
                    }
                }
            }
        }
    }
}


// --- ID Generation Helpers ---

/// Generate time-based ID (TSID pattern)
static TRANSFER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn generate_transfer_id() -> u128 {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u128;

    let seq = TRANSFER_SEQUENCE.fetch_add(1, Ordering::SeqCst) as u128;
    (timestamp_ms << 64) | seq
}

fn tb_id(type_prefix: u64, id: u64) -> u128 {
    ((type_prefix as u128) << 64) | (id as u128)
}

pub fn tb_account_id(user_id: u64, asset_id: u32) -> u128 {
    ((user_id as u128) << 64) | (asset_id as u128)
}

pub fn extract_user_id(account_id: u128) -> u64 {
    (account_id >> 64) as u64
}

pub fn extract_asset_id(account_id: u128) -> u32 {
    account_id as u32
}
