//! Transfer V2 Test Server
//!
//! Standalone HTTP server for testing transfer v2 without Aeron dependency.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{Extension, Json, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tower_http::cors::CorsLayer;

use fetcher::transfer::{
    adapters::{FundingAdapter, TradingAdapter, MockAdapter},
    TransferCoordinator, TransferDb, TransferQueue, TransferRequest, TransferState,
    TransferWorker, WorkerConfig,
};

/// Application state
struct AppState {
    coordinator: Arc<TransferCoordinator>,
    worker: Arc<TransferWorker>,
    queue: Arc<TransferQueue>,
}

#[derive(Debug, serde::Deserialize)]
struct TransferReq {
    from: String,
    to: String,
    user_id: u64,
    asset_id: u32,
    amount: u64,
}

#[derive(Debug, serde::Serialize)]
struct TransferResp {
    req_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// POST /api/v1/transfer
async fn handle_transfer(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<TransferReq>,
) -> impl IntoResponse {
    // Validate
    if payload.amount == 0 {
        return Json(TransferResp {
            req_id: "".to_string(),
            status: "failed".to_string(),
            message: None,
            error: Some("Amount must be greater than 0".to_string()),
        }).into_response();
    }

    if payload.from == payload.to {
        return Json(TransferResp {
            req_id: "".to_string(),
            status: "failed".to_string(),
            message: None,
            error: Some("Source and target cannot be the same".to_string()),
        }).into_response();
    }

    // Create transfer
    use fetcher::transfer::ServiceId;

    let from = match ServiceId::from_str(&payload.from) {
        Some(s) => s,
        None => {
            return Json(TransferResp {
                req_id: "".to_string(),
                status: "failed".to_string(),
                message: None,
                error: Some(format!("Invalid source: {}", payload.from)),
            }).into_response();
        }
    };

    let to = match ServiceId::from_str(&payload.to) {
        Some(s) => s,
        None => {
            return Json(TransferResp {
                req_id: "".to_string(),
                status: "failed".to_string(),
                message: None,
                error: Some(format!("Invalid target: {}", payload.to)),
            }).into_response();
        }
    };

    let req = TransferRequest {
        from,
        to,
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
    };

    let req_id = match state.coordinator.create(req).await {
        Ok(id) => id,
        Err(e) => {
            return Json(TransferResp {
                req_id: "".to_string(),
                status: "failed".to_string(),
                message: None,
                error: Some(e.to_string()),
            }).into_response();
        }
    };

    // Process sync
    let result = state.worker.process_now(req_id).await;

    let (status, message) = match result {
        TransferState::Committed => ("committed", None),
        TransferState::RolledBack => ("rolled_back", Some("Transfer cancelled".to_string())),
        TransferState::Failed => ("failed", Some("Transfer failed".to_string())),
        _ => {
            let _ = state.queue.try_push(req_id);
            ("pending", Some("Processing in background".to_string()))
        }
    };

    Json(TransferResp {
        req_id: req_id.to_string(),
        status: status.to_string(),
        message,
        error: None,
    }).into_response()
}

/// GET /api/v1/transfer/:req_id
async fn get_transfer(
    Extension(state): Extension<Arc<AppState>>,
    Path(req_id): Path<String>,
) -> impl IntoResponse {
    let uuid = match uuid::Uuid::parse_str(&req_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(serde_json::json!({
                "error": "Invalid req_id format"
            })).into_response();
        }
    };

    match state.coordinator.get(uuid).await {
        Ok(Some(record)) => {
            Json(serde_json::json!({
                "req_id": record.req_id.to_string(),
                "state": record.state.as_str(),
                "source": record.source.as_str(),
                "target": record.target.as_str(),
                "user_id": record.user_id,
                "asset_id": record.asset_id,
                "amount": record.amount,
                "created_at": record.created_at,
                "updated_at": record.updated_at,
                "error": record.error,
                "retry_count": record.retry_count,
            })).into_response()
        }
        Ok(None) => {
            Json(serde_json::json!({
                "error": "Transfer not found"
            })).into_response()
        }
        Err(e) => {
            Json(serde_json::json!({
                "error": e.to_string()
            })).into_response()
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    println!("🧪 Transfer V2 Test Server");
    println!("=========================");

    // Connect to ScyllaDB
    println!("📦 Connecting to ScyllaDB...");
    let session = match scylla::SessionBuilder::new()
        .known_node("127.0.0.1:9042")
        .build()
        .await
    {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("❌ Failed to connect to ScyllaDB: {}", e);
            std::process::exit(1);
        }
    };
    println!("✅ Connected to ScyllaDB");

    // Initialize schema
    println!("📋 Setting up schema...");
    let schema = r#"
        CREATE TABLE IF NOT EXISTS trading.transfers (
            req_id uuid,
            source text,
            target text,
            user_id bigint,
            asset_id int,
            amount bigint,
            state text,
            created_at bigint,
            updated_at bigint,
            error text,
            retry_count int,
            PRIMARY KEY (req_id)
        )
    "#;
    if let Err(e) = session.query(schema, &[]).await {
        eprintln!("⚠️ Schema setup warning: {}", e);
    }

    // Create components
    let db = Arc::new(TransferDb::new(session));

    // Use Mock adapters that always succeed (for testing)
    let funding = Arc::new(MockAdapter::new("funding"));
    let trading = Arc::new(MockAdapter::new("trading"));

    let coordinator = Arc::new(TransferCoordinator::new(
        db.clone(),
        funding,
        trading,
    ));

    let queue = Arc::new(TransferQueue::new(10000));
    let config = WorkerConfig::default();

    let worker = Arc::new(TransferWorker::new(
        coordinator.clone(),
        db.clone(),
        queue.clone(),
        config,
    ));

    // Spawn background worker
    let worker_clone = worker.clone();
    tokio::spawn(async move {
        worker_clone.run().await;
    });

    let state = Arc::new(AppState {
        coordinator,
        worker,
        queue,
    });

    // Build router
    let app = Router::new()
        .route("/api/v1/transfer", post(handle_transfer))
        .route("/api/v1/transfer/:req_id", get(get_transfer))
        .layer(Extension(state))
        .layer(CorsLayer::permissive());

    let port = 8080;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("🚀 Transfer V2 Test Server on http://127.0.0.1:{}", port);
    println!("");
    println!("📤 Endpoints:");
    println!("  POST /api/v1/transfer       - Create transfer");
    println!("  GET  /api/v1/transfer/:id   - Query status");

    axum::serve(
        tokio::net::TcpListener::bind(&addr).await.unwrap(),
        app.into_make_service(),
    )
    .await
    .unwrap();
}
