use std::error::Error;

use scylla::{Session, SessionBuilder};
use scylla::_macro_internal::CqlValue;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Create a new session
    let session: Session = SessionBuilder::new()
        .known_node("127.0.0.1:9042")
        .build()
        .await?;

    let network = "hal";
    let keyspace = format!("{network}_blockdata");
    // let BLOCKDATA_KEYSPACE_JSON = "blockdata_json";

    // Use a keyspace
    session.use_keyspace(keyspace, true).await?;

    // Execute a query
    let mut rows = session
        .query("SELECT cluster_name, listen_address FROM system.local", &[])
        .await?
        .rows
        .unwrap_or_default();

    // Process the results
    for row in rows {
        let cluster_name: Option<&Option<CqlValue>> = row.columns.get(0);
        let listen_address: Option<&Option<CqlValue>> = row.columns.get(1);

        match (cluster_name, listen_address) {
            (Some(cluster), Some(address)) => {
                println!("Cluster Name: {:?}, Listen Address: {:?}", cluster, address);
            }
            _ => println!("Missing cluster name or listen address"),
        }
    }


    Ok(())
}