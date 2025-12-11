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

/// Events that change balance state and need to be synced to Shadow Ledger
#[derive(Debug)]
pub enum SyncEvent {
    /// Deposit: Omnibus -> User
    Deposit {
        user_id: u64,
        asset_id: u32,
        amount: u64,
    },
    /// Withdraw: User -> Omnibus
    Withdraw {
        user_id: u64,
        asset_id: u32,
        amount: u64,
    },
    /// Lock: User -> Holding (Pending)
    Lock {
        user_id: u64,
        asset_id: u32,
        amount: u64,
        order_id: u64,
    },
    /// Unlock: Void Pending
    Unlock {
        order_id: u64,
    },
    /// Trade: Post Pending + Exchange assets + Pay Fees
    Trade {
        buyer_id: u64,
        seller_id: u64,
        base_asset: u32,
        quote_asset: u32,
        base_qty: u64,
        quote_amt: u64,
        buyer_order_id: u64,
        seller_order_id: u64,
        buyer_fee: u64,
        seller_fee: u64,
    },
}

pub struct TigerBeetleSync {
    tx: mpsc::UnboundedSender<SyncEvent>,
}

impl TigerBeetleSync {
    /// Start the background sync task and return the handle
    pub fn new(cluster_id: u128, addresses: Vec<String>) -> Result<Self, String> {
        let client = Client::new(cluster_id, addresses.join(","))
            .map_err(|e| format!("Failed to create TB client: {:?}", e))?;

        let (tx, rx) = mpsc::unbounded_channel();

        // Spawn background worker
        tokio::spawn(async move {
            Self::worker_loop(client, rx).await;
        });

        Ok(Self { tx })
    }

    /// Queue an event for synchronization
    pub fn queue(&self, event: SyncEvent) {
        if let Err(e) = self.tx.send(event) {
            log::error!("Failed to queue TB sync event: {:?}", e);
        }
    }

    /// Main worker loop processing events
    async fn worker_loop(client: Client, mut rx: mpsc::UnboundedReceiver<SyncEvent>) {
        log::info!("TigerBeetle Shadow Ledger sync started");

        // Simple implementation: process one by one for now to ensure correctness/ordering.
        // In prod, this should batch events.
        while let Some(event) = rx.recv().await {
            let transfers = match event {
                SyncEvent::Deposit { user_id, asset_id, amount } => {
                    vec![Transfer::new(generate_transfer_id())
                        .with_debit_account_id(tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id))
                        .with_credit_account_id(tb_account_id(user_id, asset_id))
                        .with_amount(amount as u128)
                        .with_ledger(TRADING_LEDGER)]
                },
                SyncEvent::Withdraw { user_id, asset_id, amount } => {
                    vec![Transfer::new(generate_transfer_id())
                        .with_debit_account_id(tb_account_id(user_id, asset_id))
                        .with_credit_account_id(tb_account_id(EXCHANGE_OMNIBUS_ID_PREFIX, asset_id))
                        .with_amount(amount as u128)
                        .with_ledger(TRADING_LEDGER)]
                },
                SyncEvent::Lock { user_id, asset_id, amount, order_id } => {
                    // Create PENDING transfer: User -> Holding
                    vec![Transfer::new(order_id as u128) // Use order_id as transfer ID for easy lookup/void
                        .with_debit_account_id(tb_account_id(user_id, asset_id))
                        .with_credit_account_id(tb_account_id(HOLDING_ACCOUNT_ID_PREFIX, asset_id))
                        .with_amount(amount as u128)
                        .with_ledger(TRADING_LEDGER)
                        .with_flags(TransferFlags::PENDING)]
                },
                SyncEvent::Unlock { order_id } => {
                    // VOID the pending transfer
                    vec![Transfer::new(generate_transfer_id())
                        .with_pending_id(order_id as u128)
                        .with_flags(TransferFlags::VOID_PENDING_TRANSFER)]
                },
                SyncEvent::Trade {
                    buyer_id, seller_id, base_asset, quote_asset,
                    base_qty, quote_amt, buyer_order_id, seller_order_id,
                    buyer_fee, seller_fee
                } => {
                    // Atomic Settlement Batch: All LINKED except the last one

                    // 1. Post Buyer (release frozen quote)
                    let post_buyer = Transfer::new(generate_transfer_id())
                        .with_pending_id(buyer_order_id as u128)
                        .with_amount((quote_amt + buyer_fee) as u128)
                        .with_flags(TransferFlags::POST_PENDING_TRANSFER | TransferFlags::LINKED);

                    // 2. Post Seller (release frozen base)
                    let post_seller = Transfer::new(generate_transfer_id())
                        .with_pending_id(seller_order_id as u128)
                        .with_amount(base_qty as u128)
                        .with_flags(TransferFlags::POST_PENDING_TRANSFER | TransferFlags::LINKED);

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
                }
            };

            // Execute transfers
            // CRITICAL: If TB rejects, it means our in-memory state (Source of Truth) is consistent
            // with an INVALID state in the Shadow Ledger. This implies a logic bug in UBS.
            match client.create_transfers(transfers).await {
                Ok(_) => {
                    log::debug!("Shadow Sync OK");
                },
                Err(e) => {
                     log::error!("CRITICAL: Shadow Ledger mismatch! TB rejected transfers: {:?}", e);
                     // PANIC to enforce consistency
                     panic!("TigerBeetle Rejected Valid UBS Operation: {:?}", e);
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

pub fn tb_account_id(user_id: u64, asset_id: u32) -> u128 {
    ((user_id as u128) << 64) | (asset_id as u128)
}

pub fn extract_user_id(account_id: u128) -> u64 {
    (account_id >> 64) as u64
}

pub fn extract_asset_id(account_id: u128) -> u32 {
    account_id as u32
}
