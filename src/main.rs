use std::env;
use std::sync::Arc;

use dotenv::dotenv;
use ethers::{
    prelude::*,
    providers::{Http, Provider},
};
use eyre::Result;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // Get the INFURA_API_KEY from the environment
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY must be set");
    let endpoint = format!("https://mainnet.infura.io/v3/{}", infura_api_key);

    // Connect to an Ethereum node (replace with your own node URL)
    let provider = Provider::<Http>::try_from(endpoint)?;
    let client = Arc::new(provider);

    // Fetch the latest block number
    let latest_block = client.get_block_number().await?;
    println!("Latest block number: {}", latest_block);

    // Fetch block data
    let block = client.get_block_with_txs(latest_block).await?;

    if let Some(block_data) = block {
        // println!("{:?}", block_data);
        println!("Block hash: {:?}", block_data.hash);
        println!("Parent hash: {:?}", block_data.parent_hash);
        println!("Timestamp: {}", block_data.timestamp);
        println!("Number of transactions: {}", block_data.transactions.len());

        for tx in block_data.transactions {
            println!("{:?}", tx)
        }
    } else {
        println!("Block not found");
    }

    Ok(())
}
