#[cfg(test)]
mod tests {
    use crate::db::OrderHistoryDb;
    use crate::ledger::{OrderStatus, OrderUpdate};

    // Helper to create a test OrderUpdate
    fn create_test_order_update(
        order_id: u64,
        user_id: u64,
        status: OrderStatus,
        filled_qty: u64,
    ) -> OrderUpdate {
        OrderUpdate {
            order_id,
            client_order_id: Some(format!("CID_{}", order_id)),
            user_id,
            symbol_id: 1,
            side: 1,       // Buy
            order_type: 1, // Limit
            status,
            price: 50000,
            qty: 1,
            filled_qty,
            avg_fill_price: if filled_qty > 0 { Some(50000) } else { None },
            rejection_reason: None,
            timestamp: 1000000,
            match_id: None,
        }
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_connect_to_scylladb() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let result = OrderHistoryDb::connect(&config).await;
        assert!(result.is_ok(), "Should connect to ScyllaDB");

        let db = result.unwrap();
        let health = db.health_check().await;
        assert!(health.is_ok(), "Health check should pass");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_upsert_active_order_new() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();
        let order = create_test_order_update(101, 1, OrderStatus::New, 0);

        let result = db.upsert_active_order(&order).await;
        assert!(result.is_ok(), "Should insert active order");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_delete_active_order() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();

        // First insert
        let order = create_test_order_update(102, 1, OrderStatus::New, 0);
        db.upsert_active_order(&order).await.unwrap();

        // Then delete
        let result = db.delete_active_order(1, 102).await;
        assert!(result.is_ok(), "Should delete active order");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_insert_order_history() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();
        let order = create_test_order_update(103, 1, OrderStatus::Filled, 1);

        let result = db.insert_order_history(&order).await;
        assert!(result.is_ok(), "Should insert order history");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_insert_order_update_stream() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();
        let order = create_test_order_update(104, 1, OrderStatus::New, 0);

        let result = db.insert_order_update_stream(&order, 1).await;
        assert!(result.is_ok(), "Should insert order update stream");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_init_user_statistics() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();

        let result = db.init_user_statistics(1, 1000000).await;
        assert!(result.is_ok(), "Should initialize user statistics");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_update_order_statistics_new() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();

        // Initialize first
        db.init_user_statistics(2, 1000000).await.unwrap();

        // Update with New status
        let result = db.update_order_statistics(2, &OrderStatus::New, 1000001).await;
        assert!(result.is_ok(), "Should update statistics for New order");
    }

    #[tokio::test]
    #[ignore] // Requires ScyllaDB running
    async fn test_full_order_lifecycle() {
        use crate::configure::ScyllaDbConfig;

        let config = ScyllaDbConfig {
            hosts: vec!["127.0.0.1:9042".to_string()],
            keyspace: "trading".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 5000,
        };

        let db = OrderHistoryDb::connect(&config).await.unwrap();
        let user_id = 3;
        let order_id = 105;

        // 1. New Order
        let order_new = create_test_order_update(order_id, user_id, OrderStatus::New, 0);
        db.upsert_active_order(&order_new).await.unwrap();
        db.insert_order_history(&order_new).await.unwrap();
        db.insert_order_update_stream(&order_new, 1).await.unwrap();
        db.init_user_statistics(user_id, order_new.timestamp).await.unwrap();
        db.update_order_statistics(user_id, &OrderStatus::New, order_new.timestamp).await.unwrap();

        // 2. Partially Filled
        let order_partial =
            create_test_order_update(order_id, user_id, OrderStatus::PartiallyFilled, 1);
        db.upsert_active_order(&order_partial).await.unwrap();
        db.insert_order_history(&order_partial).await.unwrap();
        db.insert_order_update_stream(&order_partial, 2).await.unwrap();

        // 3. Filled
        let order_filled = create_test_order_update(order_id, user_id, OrderStatus::Filled, 2);
        db.delete_active_order(user_id, order_id).await.unwrap();
        db.insert_order_history(&order_filled).await.unwrap();
        db.insert_order_update_stream(&order_filled, 3).await.unwrap();
        db.update_order_statistics(user_id, &OrderStatus::Filled, order_filled.timestamp)
            .await
            .unwrap();

        // All operations should succeed
        println!("✅ Full order lifecycle test passed");
    }

    #[test]
    fn test_order_update_creation() {
        let order = create_test_order_update(1, 1, OrderStatus::New, 0);

        assert_eq!(order.order_id, 1);
        assert_eq!(order.user_id, 1);
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.filled_qty, 0);
        assert_eq!(order.symbol_id, 1);
        assert_eq!(order.price, 50000);
        assert_eq!(order.qty, 1);
    }

    #[test]
    fn test_order_update_with_filled_qty() {
        let order = create_test_order_update(2, 1, OrderStatus::PartiallyFilled, 1);

        assert_eq!(order.filled_qty, 1);
        assert_eq!(order.avg_fill_price, Some(50000));
        assert_eq!(order.status, OrderStatus::PartiallyFilled);
    }

    #[test]
    fn test_order_statuses() {
        let statuses = vec![
            OrderStatus::New,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::Cancelled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
        ];

        for status in statuses {
            let order = create_test_order_update(1, 1, status.clone(), 0);
            assert_eq!(order.status, status);
        }
    }
}
