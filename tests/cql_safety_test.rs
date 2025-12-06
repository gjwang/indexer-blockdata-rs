use scylla::{Session, SessionBuilder};
use anyhow::Result;

async fn setup_db() -> Result<Session> {
    SessionBuilder::new()
        .known_node("localhost:9042")
        .build()
        .await
        .map_err(|e| anyhow::anyhow!(e))
}

#[tokio::test]
#[ignore] // Integration test - requires ScyllaDB running
async fn test_invalid_version_increment_cql() -> Result<()> {
    let session = setup_db().await?;

    // This syntax caused the "Invalid operation" error:
    // SET version = version + 1 (on standard bigint column)
    let bad_cql = "UPDATE settlement.user_balances SET version = version + 1 WHERE user_id = 9999 AND asset_id = 1";

    let result = session.query(bad_cql, ()).await;

    // Verify it fails
    assert!(result.is_err(), "Should fail due to invalid syntax (+= on non-counter)");

    let err = result.unwrap_err();
    let msg = err.to_string();
    println!("Captured expected error: {}", msg);

    // Check for "Invalid operation" or similar
    // Note: The exact message depends on ScyllaDB version, but "Invalid operation" was seen in logs.
    assert!(msg.to_lowercase().contains("invalid operation") || msg.contains("SyntaxException"),
            "Unexpected error message: {}", msg);

    Ok(())
}

#[tokio::test]
#[ignore] // Integration test - requires ScyllaDB running
async fn test_valid_version_update_cql() -> Result<()> {
    let session = setup_db().await?;

    // Valid syntax: Absolute value usage
    // SET version = ?
    let good_cql = "UPDATE settlement.user_balances SET version = ? WHERE user_id = 9999 AND asset_id = 1";

    let result = session.query(good_cql, (100i64,)).await;

    assert!(result.is_ok(), "Should succeed with valid syntax");

    Ok(())
}
