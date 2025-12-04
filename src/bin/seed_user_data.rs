use chrono::Utc;
use fetcher::db::SettlementDb;
use fetcher::ledger::{LedgerEvent, MatchExecData};

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    let db = if let Some(scylla_config) = &config.scylladb {
        SettlementDb::connect(scylla_config).await.expect("Failed to connect to DB")
    } else {
        panic!("ScyllaDB config missing");
    };

    let user_id = 1001;
    let base_asset = 1; // BTC
    let quote_asset = 2; // USDT

    println!("Seeding data for user {}", user_id);

    // 1. Deposit BTC (Asset 1)
    let deposit_btc = LedgerEvent {
        user_id,
        sequence_id: 1,
        event_type: "DEPOSIT".to_string(),
        amount: 150_000_000, // 1.5 BTC
        currency: base_asset,
        related_id: 0,
        created_at: Utc::now().timestamp_millis(),
    };
    db.insert_ledger_event(&deposit_btc).await.expect("Failed to insert BTC deposit");

    // 2. Deposit USDT (Asset 2)
    let deposit_usdt = LedgerEvent {
        user_id,
        sequence_id: 2,
        event_type: "DEPOSIT".to_string(),
        amount: 1_000_000_000_000, // 10,000 USDT
        currency: quote_asset,
        related_id: 0,
        created_at: Utc::now().timestamp_millis(),
    };
    db.insert_ledger_event(&deposit_usdt).await.expect("Failed to insert USDT deposit");

    // 3. Insert a Trade (Buy BTC)
    let trade = MatchExecData {
        trade_id: 100,
        buy_order_id: 5001,
        sell_order_id: 6001,
        buyer_user_id: user_id,
        seller_user_id: 2000,
        price: 50_000_000_000_000, // 50,000.00
        quantity: 10_000_000,      // 0.1 BTC
        base_asset: base_asset,
        quote_asset: quote_asset,
        buyer_refund: 0,
        seller_refund: 0,
        match_seq: 10,
        output_sequence: 10,
        settled_at: Utc::now().timestamp_millis() as u64,
        buyer_quote_version: 0,
        buyer_base_version: 0,
        seller_base_version: 0,
        seller_quote_version: 0,
    };

    db.insert_trade(&trade).await.expect("Failed to insert trade");

    println!("✅ Data seeded successfully!");
}
