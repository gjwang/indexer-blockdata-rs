use std::env;
// use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use dotenv::dotenv;
use ethers::{
    prelude::*,
    providers::{Http, Provider},
};
use eyre::Result;
use log::info;
// use serde_json::{json, Value};
use tokio::time::sleep;

use fetcher::compressor::decompress_json;
use fetcher::logger;
use fetcher::s3_service::S3Service;
use fetcher::simple_kv_storage::SledDb;

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

    let db_name = "config_db/indexer";
    let kv_db = SledDb::new(db_name)?;

    let kv_blk_number_begin_key = "indexer::block_number_begin";
    let kv_blk_number_end_key = "indexer::block_number_end";
    if block_number_begin < 0 {
        // block_number_begin = kv_db.get(kv_blk_number_begin_key, 0);
    } else {
        kv_db.insert(kv_blk_number_begin_key, block_number_begin)?;
    }
    block_number_begin = kv_db.get(kv_blk_number_begin_key, 0);

    let mut block_number_end;
    if _block_number_end == -1 {
        //use LatestBlockNumber value as block_number_end
        block_number_end = i64::try_from(client.get_block_number().await?)?;
        info!("LatestBlockNumber: {}", block_number_end);
        kv_db.insert(kv_blk_number_end_key, block_number_end)?;
    } else if _block_number_end == -2 {
        //use local storage value as block_number_end
        block_number_end = kv_db.get(kv_blk_number_end_key, -2);
        info!("use local block_number_end: {block_number_end}");
    } else {
        //use input value as block_number_end
        kv_db.insert(kv_blk_number_end_key, _block_number_end)?;
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

    let s3_service =
        S3Service::new(bucket_name, region, endpoint, &aws_access_key_id, &aws_secret_access_key)?;

    loop {
        block_number_begin = kv_db.get(kv_blk_number_begin_key, 0);
        block_number_end = kv_db.get(kv_blk_number_end_key, -1);

        let delay_blocks = block_number_end - block_number_begin;
        info!("index delay_blocks={delay_blocks} block_number_begin={block_number_begin} block_number_end={block_number_end}");

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

        let block_number = if is_reverse_indexing { block_number_end } else { block_number_begin };

        let key = format!("{block_number}.json.gz");
        let block_data = s3_service.get_object(&key).await;
        match block_data {
            Ok(data) => {
                println!("get_object {key} ✅");
                let _decompressed_json = decompress_json(&data)?; // Decompress to verifyJSON data
                println!("Decompressed block: {key}");
                //TODO save into scylla
            }
            Err(e) => {
                println!("get_object block:{key} Error: {:?}", e);
                sleep(Duration::from_secs(10)).await;
                continue;
            }
        }

        if !is_reverse_indexing {
            block_number_begin += 1;
            kv_db.insert(kv_blk_number_begin_key, block_number_begin)?;
        } else {
            block_number_end -= 1;
            kv_db.insert(kv_blk_number_end_key, block_number_end)?;
        }
    }
}
