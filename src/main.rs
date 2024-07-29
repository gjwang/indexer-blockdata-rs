use serde::{Deserialize, Serialize};
use serde_json::json;

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
    // Ethereum JSON-RPC endpoint (replace with your preferred node or service)
    let endpoint = "https://mainnet.infura.io/v3/YOUR_INFURA_PROJECT_ID";

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