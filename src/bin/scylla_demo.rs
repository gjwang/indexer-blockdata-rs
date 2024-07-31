use anyhow::Result;

use indexerblockdata::scylla_service::example_usage;

#[tokio::main]
async fn main() -> Result<()> {
    example_usage().await.expect("TODO: panic message");

    Ok(())
}
