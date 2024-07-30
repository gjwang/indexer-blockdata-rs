use std::env;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use dotenv::dotenv;
use ethers::{
    prelude::*,
    providers::{Http, Provider},
};
use eyre::Result;
use log::{error, info};
use log4rs;
use tokio::time::sleep;

use simple_kv_storage::SledDb;

mod logger;
mod configure;
mod simple_kv_storage;
#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "-1")]
    block_number_begin: i64,
    #[clap(long, default_value = "-1")]
    block_number_end: i64,
    #[clap(long, action = clap::ArgAction::Set, default_value_t = false)]
    is_reverse_indexing: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let args = Args::parse();
    let mut block_number_begin = args.block_number_begin;
    let _block_number_end = args.block_number_end;
    let is_reverse_indexing = args.is_reverse_indexing;
    println!("block_number_begin={block_number_begin} block_number_end={_block_number_end} is_reverse_indexing={is_reverse_indexing}");

    logger::setup_logger().expect("Failed to set up logger");


    // Get the INFURA_API_KEY from the environment
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY must be set");
    let endpoint = format!("https://mainnet.infura.io/v3/{infura_api_key}");

    // Connect to an Ethereum node (replace with your own node URL)
    let provider = Provider::<Http>::try_from(endpoint)?;
    let client = Arc::new(provider);


    let db_name = "config_db";
    let kv_db = SledDb::new(db_name)?;

    if block_number_begin < 0 {
        block_number_begin = kv_db.get("block_number_begin", 0);
    } else {
        kv_db.insert("block_number_begin", block_number_begin)?;
    }
    block_number_begin = kv_db.get("block_number_begin", 0);


    let mut block_number_end;
    if _block_number_end == -1 {
        //use LatestBlockNumber value as block_number_end
        block_number_end = i64::try_from(client.get_block_number().await?)?;
        info!("LatestBlockNumber: {}", block_number_end);
        kv_db.insert("block_number_end", block_number_end)?;
    } else if _block_number_end == -2 {
        //use local storage value as block_number_end
        block_number_end = kv_db.get("block_number_end", -2);
        info!("use local block_number_end: {block_number_end}");
    } else {
        //use input value as block_number_end
        kv_db.insert("block_number_end", _block_number_end)?;
        block_number_end = _block_number_end;
    }


    println!("block_number_begin: {block_number_begin} _block_number_end={_block_number_end} block_number_end={block_number_end}");

    loop {
        block_number_begin = kv_db.get("block_number_begin", 0);

        if !is_reverse_indexing {
            block_number_end = i64::try_from(client.get_block_number().await?)?;
            info!("LatestBlockNumber: {}", block_number_end);
        } else {
            block_number_end = kv_db.get("block_number_end", -1);
        }

        let delay_blocks = block_number_end - block_number_begin;
        info!("delay_blocks={delay_blocks} block_number_begin={block_number_begin} block_number_end={block_number_end}");

        if delay_blocks <= 0 {
            let duration = Duration::from_secs(5);
            info!("catchup the latest_block_number={block_number_end} sleep {:?}", duration);
            if is_reverse_indexing {
                info!("Finished all");
                return Ok(());
            }
            sleep(duration).await;
            continue;
        }

        let block_number;
        if is_reverse_indexing {
            block_number = block_number_end;
        } else {
            block_number = block_number_begin;
        }

        // Fetch block data
        let block = client.get_block_with_txs(U64::from(block_number_begin)).await?;
        if let Some(block_data) = block {
            info!("BlockNumber: {:?}, hash:{:?}", block_data.number, block_data.hash);
            info!("Parent hash: {:?}", block_data.parent_hash);
            info!("Timestamp: {}", block_data.timestamp);
            info!("Number of transactions: {}", block_data.transactions.len());

            // for tx in block_data.transactions {
            //     info!("{:?} {} -> {:?}", tx.hash, tx.from, tx.to);
            // }
        } else {
            error!("Block not found");
        }

        if !is_reverse_indexing {
            block_number_begin += 1;
            kv_db.insert("block_number_begin", block_number_begin)?;
        } else {
            block_number_end -= 1;
            kv_db.insert("block_number_end", block_number_end)?;
        }
    }
}
