use std::convert::Infallible;
use std::env;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use cached::proc_macro::cached;
use cached::SizedCache;
use clap::Parser;
use dotenv::dotenv;
use ethers::{
    prelude::*,
    providers::{Http, Provider},
};
use eyre::Result;
use log::{error, info};
use serde_json::{json, Value};
use tokio::time::sleep;

use fetcher::s3_service::S3Service;
use simple_kv_storage::SledDb;

use crate::compressor::{compress_json, decompress_json};

mod compressor;
mod configure;
mod logger;
mod scylla_service;
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

async fn get_block_data(
    client: &Provider<Http>,
    block_number_begin: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    let filter = Filter::new()
        .from_block(block_number_begin)
        .to_block(block_number_begin);

    let block = client
        .get_block_with_txs(U64::from(block_number_begin))
        .await?;
    // println!("block= {:?}", block);
    let logs = client.get_logs(&filter).await?;

    let mut block_json: Value = serde_json::to_value(&block)?;

    // Add logs to the block JSON
    if let Value::Object(ref mut map) = block_json {
        map.insert("logs".to_string(), json!(logs));
    }

    Ok(block_json)
}

#[cached(time = 10)]
async fn get_latest_block_number(client: Arc<Provider<Http>>) -> Result<i64, Box<dyn std::error::Error>> {
    let block_number_end = i64::try_from(client.get_block_number().await?)?;
    info!("get_latest_block_number block_number_end={block_number_end}");
    Ok(block_number_end)
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

    let db_name = "config_db/fetcher";
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
        // block_number_end = i64::try_from(client.get_block_number().await?)?;
        block_number_end = get_latest_block_number(client.clone()).await?;
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

    let endpoint = "http://localhost:9000"; // Use this for MinIO, comment out for S3
    let bucket_name = "my-bucket2";
    let region = "us-east-1";
    let aws_access_key_id = env::var("S3_ACCESS_KEY_ID").expect("S3_ACCESS_KEY_ID must be set");
    let aws_secret_access_key =
        env::var("S3_SECRET_ACCESS_KEY").expect("S3_SECRET_ACCESS_KEY must be set");
    println!(
        "endpoint={endpoint}, bucket_name={bucket_name}, aws_access_key_id={aws_access_key_id}"
    );

    let s3_service = S3Service::new(
        &bucket_name,
        &region,
        &endpoint,
        &aws_access_key_id,
        &aws_secret_access_key,
    )?;

    loop {
        block_number_begin = kv_db.get("block_number_begin", 0);

        if !is_reverse_indexing {
            // block_number_end = i64::try_from(client.get_block_number().await?)?;
            block_number_end = get_latest_block_number(client.clone()).await?;
            info!("LatestBlockNumber: {}", block_number_end);
        } else {
            block_number_end = kv_db.get("block_number_end", -1);
        }

        let delay_blocks = block_number_end - block_number_begin;
        info!("delay_blocks={delay_blocks} block_number_begin={block_number_begin} block_number_end={block_number_end}");

        if delay_blocks <= 0 {
            let duration = Duration::from_secs(5);
            info!(
                "catchup the latest_block_number={block_number_end} sleep {:?}",
                duration
            );
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

        let block_data = get_block_data(&client, block_number_begin as u64).await?;
        // println!("{}", serde_json::to_string_pretty(&block_data)?);

        let number = block_data["number"].as_str().unwrap();
        println!("BlockNumber: {}", number);
        let hash = block_data["hash"].as_str().unwrap();
        println!("Block Hash: {}", hash);

        let compressed_data = compress_json(&block_data)?;
        let key = format!("{block_number}.json.gz");
        s3_service.upload_object(&key, compressed_data).await?;
        println!("upload block data {key} to S3 success ✅");

        if !is_reverse_indexing {
            block_number_begin += 1;
            kv_db.insert("block_number_begin", block_number_begin)?;
        } else {
            block_number_end -= 1;
            kv_db.insert("block_number_end", block_number_end)?;
        }
    }
}
