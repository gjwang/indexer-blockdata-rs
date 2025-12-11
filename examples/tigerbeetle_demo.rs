//! TigerBeetle Full Architecture Demo
//!
//! This example demonstrates the complete Shadow Ledger lifecycle:
//! 1. Account Setup (User, Omnibus, Holding, Revenue)
//! 2. Deposits (Omnibus -> User)
//! 3. Order Placement (Lock Funds: User -> Holding [PENDING])
//! 4. Trade Execution (Atomic Batch: Post Funds + Swap Assets + Pay Fees)
//! 5. Verification of Final State
//!
//! Run TigerBeetle first:
//!   ./scripts/setup_tigerbeetle.sh single
//!   docker-compose -f docker-compose.tigerbeetle.yml up -d
//!
//! Then run this example:
//!   cargo run --example tigerbeetle_demo

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tigerbeetle_unofficial::{Account, Client, Transfer};
use tigerbeetle_unofficial::account::Flags as AccountFlags;
use tigerbeetle_unofficial::transfer::Flags as TransferFlags;

// =============================================================================
// Configuration & Constants
// =============================================================================

const CLUSTER_ID: u128 = 0;
const TB_ADDRESS: &str = "127.0.0.1:3000";

const TRADING_LEDGER: u32 = 1;

// Assets
const BTC: u32 = 1;
const USDT: u32 = 2;

// Scaling
const BTC_SCALE: u128 = 100_000_000;
const USDT_SCALE: u128 = 1_000_000;

// Special Account Prefixes (High 64 bits)
const EXCHANGE_OMNIBUS_PREFIX: u64 = u64::MAX;
const HOLDING_PREFIX: u64 = u64::MAX - 1;
const REVENUE_PREFIX: u64 = u64::MAX - 2;

// Users
const BUYER_ID: u64 = 1001;
const SELLER_ID: u64 = 1002;

// =============================================================================
// ID Generators
// =============================================================================

static TRANSFER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn tb_account_id(user_id: u64, asset_id: u32) -> u128 {
    ((user_id as u128) << 64) | (asset_id as u128)
}

fn generate_id() -> u128 {
    let timestamp_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u128;
    let sequence = TRANSFER_SEQUENCE.fetch_add(1, Ordering::SeqCst) as u128;
    (timestamp_ms << 64) | sequence
}

fn format_amt(amount: u128, asset: u32) -> String {
    let (scale, dec) = if asset == BTC { (BTC_SCALE, 8) } else { (USDT_SCALE, 6) };
    format!("{}.{:0width$}", amount / scale, amount % scale, width=dec)
}

// =============================================================================
// Operations
// =============================================================================

async fn create_accounts(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let mut accounts = Vec::new();

    // Helper to add account to batch
    // Omnibus/Holding/Revenue allow negative (liabilities/equity representation)
    // or just Credits/Debits.
    // Usually Omnibus = Credits must not exceed Debits (Pure Liability)?
    // Let's use standard: User accounts cannot go negative.
    // Special accounts: Allow whatever context needs.
    // Simplifying: All accounts DEBITS_MUST_NOT_EXCEED_CREDITS except OMNIBUS which is source of funds.
    // Actually, Omnibus creates money by having "Credits Must Not Exceed Debits" (it issues credits)?
    // Let's stick to: Omnibus is the source. It can go negative?
    // Ideally, we mint money by creating valid transfers.
    // For this demo, let's allow Omnibus to have net negative (it holds liabilities = user deposits).

    let assets = [BTC, USDT];

    // 1. Omnibus Accounts (Source of User Deposits) - Allow Negative
    for asset in assets {
        accounts.push(Account::new(tb_account_id(EXCHANGE_OMNIBUS_PREFIX, asset), TRADING_LEDGER, asset as u16)
            .with_flags(AccountFlags::CREDITS_MUST_NOT_EXCEED_DEBITS)); // Net Debit Balance allowed (Liability)
    }

    // 2. Holding Accounts (Holds Frozen Funds)
    for asset in assets {
        accounts.push(Account::new(tb_account_id(HOLDING_PREFIX, asset), TRADING_LEDGER, asset as u16)
            .with_flags(AccountFlags::DEBITS_MUST_NOT_EXCEED_CREDITS));
    }

    // 3. Revenue Accounts (Fee Collection)
    for asset in assets {
        accounts.push(Account::new(tb_account_id(REVENUE_PREFIX, asset), TRADING_LEDGER, asset as u16)
            .with_flags(AccountFlags::DEBITS_MUST_NOT_EXCEED_CREDITS));
    }

    // 4. Users
    for user in [BUYER_ID, SELLER_ID] {
        for asset in assets {
             accounts.push(Account::new(tb_account_id(user, asset), TRADING_LEDGER, asset as u16)
                .with_flags(AccountFlags::DEBITS_MUST_NOT_EXCEED_CREDITS));
        }
    }

    match client.create_accounts(accounts).await {
        Ok(_) => println!("✅ Accounts created successfully (or exist)"),
        Err(e) => if !format!("{:?}", e).contains("Exists") { return Err(format!("{:?}", e).into()); }
    }
    Ok(())
}

async fn deposit(client: &Client, user: u64, asset: u32, amount: u128) -> Result<(), Box<dyn std::error::Error>> {
    let transfer = Transfer::new(generate_id())
        .with_debit_account_id(tb_account_id(EXCHANGE_OMNIBUS_PREFIX, asset))
        .with_credit_account_id(tb_account_id(user, asset))
        .with_amount(amount)
        .with_ledger(TRADING_LEDGER)
        .with_code(1); // Deposit

    client.create_transfers(vec![transfer]).await.map_err(|e| format!("{:?}", e).into()).map(|_| ())
}

async fn lock_funds(client: &Client, user: u64, asset: u32, amount: u128, order_id: u64) -> Result<(), Box<dyn std::error::Error>> {
    // Lock = PENDING Transfer: User -> Holding
    let transfer = Transfer::new(order_id as u128) // Use order_id as transfer ID
        .with_debit_account_id(tb_account_id(user, asset))
        .with_credit_account_id(tb_account_id(HOLDING_PREFIX, asset))
        .with_amount(amount)
        .with_ledger(TRADING_LEDGER)
        .with_flags(TransferFlags::PENDING)
        .with_code(10); // Lock

    client.create_transfers(vec![transfer]).await.map_err(|e| format!("{:?}", e).into()).map(|_| ())
}

async fn settle_atomic(
    client: &Client,
    buyer_order: u64,
    seller_order: u64,
    buyer_id: u64,
    seller_id: u64,
    base_qty: u128,
    quote_amt: u128, // Amount passed to seller
    buyer_fee: u128,
    seller_fee: u128
) -> Result<(), Box<dyn std::error::Error>> {

    // 1. Post Buyer (Release frozen Quote) - LINKED
    // We must POST the total amount we intend to spend from the lock (Principal + Fee)
    let post_buyer = Transfer::new(generate_id())
        .with_pending_id(buyer_order as u128)
        .with_amount(quote_amt + buyer_fee)
        .with_flags(TransferFlags::POST_PENDING_TRANSFER | TransferFlags::LINKED);

    // 2. Post Seller (Release frozen Base) - LINKED
    let post_seller = Transfer::new(generate_id())
        .with_pending_id(seller_order as u128)
        .with_amount(base_qty)
        .with_flags(TransferFlags::POST_PENDING_TRANSFER | TransferFlags::LINKED);

    // 3. Principal Swap: Seller -> Buyer (Base) - LINKED
    // Source is HOLDING because Seller locked Base there
    let base_tx = Transfer::new(generate_id())
        .with_debit_account_id(tb_account_id(HOLDING_PREFIX, BTC))
        .with_credit_account_id(tb_account_id(buyer_id, BTC))
        .with_amount(base_qty)
        .with_ledger(TRADING_LEDGER)
        .with_code(2) // Trade
        .with_flags(TransferFlags::LINKED);

    // 4. Principal Swap: Buyer -> Seller (Quote) - LINKED
    // Source is HOLDING because Buyer locked Quote there
    let quote_tx = Transfer::new(generate_id())
        .with_debit_account_id(tb_account_id(HOLDING_PREFIX, USDT))
        .with_credit_account_id(tb_account_id(seller_id, USDT))
        .with_amount(quote_amt)
        .with_ledger(TRADING_LEDGER)
        .with_code(2) // Trade
        .with_flags(TransferFlags::LINKED);

    // 5. Fee: Buyer -> Revenue (Quote) - LINKED
    // Source is HOLDING because Buyer locked Quote (Price + Fee) there
    let fee_buyer_tx = Transfer::new(generate_id())
        .with_debit_account_id(tb_account_id(HOLDING_PREFIX, USDT))
        .with_credit_account_id(tb_account_id(REVENUE_PREFIX, USDT))
        .with_amount(buyer_fee)
        .with_ledger(TRADING_LEDGER)
        .with_code(3) // Fee
        .with_flags(TransferFlags::LINKED);

    // 6. Fee: Seller -> Revenue (Quote) - LAST (No Linked)
    // Source is SELLER because Seller pays from derived proceeds (Quote)
    let fee_seller_tx = Transfer::new(generate_id())
        .with_debit_account_id(tb_account_id(seller_id, USDT))
        .with_credit_account_id(tb_account_id(REVENUE_PREFIX, USDT))
        .with_amount(seller_fee)
        .with_ledger(TRADING_LEDGER)
        .with_code(3); // Fee

    let batch = vec![post_buyer, post_seller, base_tx, quote_tx, fee_buyer_tx, fee_seller_tx];

    client.create_transfers(batch).await.map_err(|e| format!("{:?}", e).into()).map(|_| ())
}

// Helper to print balances
async fn print_user_balance(client: &Client, user: u64, name: &str) {
    println!("\n👤 User {}: {}", user, name);
    println!("   {: <5} | {: >15} | {: >15} | {: >15}", "Asset", "Total", "Available", "Frozen");
    println!("   {}", "-".repeat(60));

    for asset in [BTC, USDT] {
        let id = tb_account_id(user, asset);
        let accs = client.lookup_accounts(vec![id]).await.unwrap();
        if let Some(a) = accs.first() {
            let total = a.credits_posted() - a.debits_posted();
            let frozen = a.debits_pending();
            let avail = total - frozen;
            println!("   {: <5} | {: >15} | {: >15} | {: >15}",
                if asset==BTC{"BTC"}else{"USDT"},
                format_amt(total, asset),
                format_amt(avail, asset),
                format_amt(frozen, asset));
        }
    }
}
async fn print_revenue(client: &Client) {
    println!("\n💰 Revenue (Platform Fees):");
    for asset in [BTC, USDT] {
        let id = tb_account_id(REVENUE_PREFIX, asset);
        let accs = client.lookup_accounts(vec![id]).await.unwrap();
         if let Some(a) = accs.first() {
             let total = a.credits_posted() - a.debits_posted(); // Revenue is credits
             if total > 0 {
                println!("   {: <5}: {: >15}", if asset==BTC{"BTC"}else{"USDT"}, format_amt(total, asset));
             }
         }
    }
}


// =============================================================================
// MAIN
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║         TigerBeetle Shadow Ledger Demo                         ║");
    println!("║                (Full Atomic Lifecycle)                         ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let client = Client::new(CLUSTER_ID, TB_ADDRESS)?;

    // 1. Setup
    println!("--- Step 1: Account Setup ---");
    create_accounts(&client).await?;

    // 2. Deposits
    println!("\n--- Step 2: Deposits ---");
    // Buyer: 50k USDT
    // Seller: 2 BTC
    println!("-> User {} deposits 50,000 USDT", BUYER_ID);
    deposit(&client, BUYER_ID, USDT, 50_000 * USDT_SCALE).await?;

    println!("-> User {} deposits 2.0 BTC", SELLER_ID);
    deposit(&client, SELLER_ID, BTC, 2 * BTC_SCALE).await?;

    print_user_balance(&client, BUYER_ID, "Buyer").await;
    print_user_balance(&client, SELLER_ID, "Seller").await;

    // 3. Locks
    println!("\n--- Step 3: Order Locks (Frozen Funds) ---");
    // Buyer places order: Buy 1 BTC @ 40k. Needs 40k USDT + 10 USDT Fee. Locks 40,010.
    // Use dynamic ID to avoid "Exists" error on re-runs
    let buyer_order_id = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() % 1_000_000) as u64 + 1000;
    println!("-> Buyer Order #{}: Locking 40,010 USDT (Price + Fee)", buyer_order_id);
    lock_funds(&client, BUYER_ID, USDT, 40_010 * USDT_SCALE, buyer_order_id).await?;

    // Seller places order: Sell 1 BTC. Locks 1 BTC.
    let seller_order_id = buyer_order_id + 1;
    println!("-> Seller Order #{}: Locking 1.0 BTC", seller_order_id);
    lock_funds(&client, SELLER_ID, BTC, 1 * BTC_SCALE, seller_order_id).await?;

    println!("\n[State After Locks]");
    print_user_balance(&client, BUYER_ID, "Buyer").await;
    print_user_balance(&client, SELLER_ID, "Seller").await;

    // 4. Atomic Trade
    println!("\n--- Step 4: Atomic Settlement (6-Step Batch w/ Fees) ---");
    println!("   Match: 1 BTC @ 40,000 USDT");
    println!("   Fees: Buyer pays 10 USDT, Seller pays 20 USDT");

    settle_atomic(
        &client,
        buyer_order_id,
        seller_order_id,
        BUYER_ID,
        SELLER_ID,
        1 * BTC_SCALE, // Base Qty
        40_000 * USDT_SCALE, // Quote Amt
        10 * USDT_SCALE, // Buyer Fee
        20 * USDT_SCALE  // Seller Fee
    ).await?;

    println!("✅ Trade Settled Atomically!");

    println!("\n[Final State]");
    print_user_balance(&client, BUYER_ID, "Buyer").await;
    print_user_balance(&client, SELLER_ID, "Seller").await;
    print_revenue(&client).await;

    // 5. Verify Fail Case
    println!("\n--- Step 5: Safety Check (Double Spend) ---");
    println!("-> Attempts to lock 40k USDT again (only ~10k avail)");
    match lock_funds(&client, BUYER_ID, USDT, 40_000 * USDT_SCALE, 9999).await {
        Ok(_) => println!("❌ ERROR: Lock succeeded but should have failed!"),
        Err(e) => println!("✅ Lock Correctly Rejected: {}", e)
    }

    Ok(())
}
