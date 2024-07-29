use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use dotenv::dotenv;

#[derive(Debug, Serialize, Deserialize)]
struct BlockData {
    number: String,
    hash: String,
    parentHash: String,
    nonce: String,
    sha3Uncles: String,
    logsBloom: String,
    transactionsRoot: String,
    stateRoot: String,
    receiptsRoot: String,
    miner: String,
    difficulty: String,
    totalDifficulty: String,
    extraData: String,
    size: String,
    gasLimit: String,
    gasUsed: String,
    timestamp: String,
    // Add more fields as needed
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    result: BlockData,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    // Get the INFURA_API_KEY from the environment
    let infura_api_key = env::var("INFURA_API_KEY").expect("INFURA_API_KEY must be set");

    // Ethereum JSON-RPC endpoint
    let endpoint = format!("https://mainnet.infura.io/v3/{}", infura_api_key);


    // Create a client
    let client = reqwest::Client::new();

    // Prepare the JSON-RPC request
    let request_body = json!({
        "jsonrpc": "2.0",
        "method": "eth_getBlockByNumber",
        "params": ["latest", false],
        "id": 1
    });

    // Send the request
    let response = client
        .post(endpoint)
        .json(&request_body)
        .send()
        .await?
        .json::<JsonRpcResponse>()
        .await?;

    // Print the block data
    println!("Latest Ethereum Block Data:");
    println!("Block Number: {}", u64::from_str_radix(&response.result.number[2..], 16)?);
    println!("Block Hash: {}", response.result.hash);
    println!("Parent Hash: {}", response.result.parentHash);
    println!("Timestamp: {}", u64::from_str_radix(&response.result.timestamp[2..], 16)?);
    println!("Gas Used: {}", u64::from_str_radix(&response.result.gasUsed[2..], 16)?);
    println!("Gas Limit: {}", u64::from_str_radix(&response.result.gasLimit[2..], 16)?);

    Ok(())
}