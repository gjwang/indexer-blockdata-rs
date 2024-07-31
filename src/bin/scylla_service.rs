use std::sync::Arc;

use anyhow::Result;
use scylla::{Session, SessionBuilder};
use scylla::_macro_internal::{CqlValue, Row};
use scylla::transport::Node;
use tokio::sync::Mutex;

pub struct ScyllaService {
    session: Arc<Mutex<Option<Session>>>,
}

impl ScyllaService {
    pub fn new() -> Self {
        ScyllaService {
            session: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn connect(&self, node: &str) -> Result<()> {
        let session = SessionBuilder::new().known_node(node).build().await?;
        let mut session_guard = self.session.lock().await;
        *session_guard = Some(session);
        Ok(())
    }

    pub async fn execute_query(&self, query: &str) -> Result<Vec<Row>> {
        let session_guard = self.session.lock().await;
        if let Some(session) = &*session_guard {
            let rows = session.query(query, &[]).await?.rows;
            Ok(rows.unwrap_or_default())
        } else {
            Err(anyhow::anyhow!("Not connected to Scylla"))
        }
    }


    pub async fn get_cluster_info(&self) -> Result<Vec<Arc<Node>>> {
        let session_guard = self.session.lock().await;
        if let Some(session) = &*session_guard {
            let cluster_data = session.get_cluster_data();
            let nodes_info = cluster_data.get_nodes_info().to_vec();
            Ok(nodes_info)
        } else {
            Err(anyhow::anyhow!("Not connected to Scylla"))
        }
    }
}

// Usage example
async fn example_usage() -> Result<()> {
    let scylla_service = ScyllaService::new();
    scylla_service.connect("127.0.0.1:9042").await?;

    scylla_service.execute_query("CREATE KEYSPACE IF NOT EXISTS test_keyspace WITH REPLICATION = { 'class' : 'SimpleStrategy', 'replication_factor' : 1 }").await?;

    let nodes_info = scylla_service.get_cluster_info().await?;
    println!("nodes_info:{:?}", nodes_info);


    let stmt = "SELECT cluster_name, listen_address FROM system.local";
    // Use the service to execute queries
    let rows = scylla_service.execute_query(stmt).await?;
    println!("{:?}", rows);
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

#[tokio::main]
async fn main() -> Result<()> {
    example_usage().await.expect("TODO: panic message");

    Ok(())
}

