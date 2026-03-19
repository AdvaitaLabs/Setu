//! HTTP Server abstractions for Solver
//!
//! Provides the server-side framework for handling Validator requests.

use crate::http::types::{
    ExecuteTaskRequest, ExecuteTaskResponse,
    ExecuteBatchRequest, ExecuteBatchResponse,
    HealthResponse, SolverInfoResponse,
};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use tracing::{error, info};

/// Handler trait for processing SolverTask requests
///
/// Implement this trait to define custom task execution logic.
/// The HTTP server will call these methods when requests arrive.
#[async_trait]
pub trait SolverHttpHandler: Send + Sync + 'static {
    /// Execute a solver task
    ///
    /// This is called for POST /api/v1/execute-task requests.
    async fn execute_task(&self, request: ExecuteTaskRequest) -> ExecuteTaskResponse;

    /// Health check
    ///
    /// This is called for GET /health requests.
    async fn health(&self) -> HealthResponse;

    /// Solver info
    ///
    /// This is called for GET /api/v1/info requests.
    async fn info(&self) -> SolverInfoResponse;

    /// Execute a batch of solver tasks
    ///
    /// This is called for POST /api/v1/execute-task-batch requests.
    /// Default implementation: sequential fallback (for backward compatibility).
    async fn execute_task_batch(&self, request: ExecuteBatchRequest) -> ExecuteBatchResponse {
        let start = std::time::Instant::now();
        let batch_id = request.batch_id.clone();
        let total = request.tasks.len();
        let mut results = Vec::with_capacity(total);
        let mut success_count = 0;

        for task_req in request.tasks {
            let resp = self.execute_task(task_req).await;
            if resp.success { success_count += 1; }
            results.push(resp);
        }

        ExecuteBatchResponse {
            all_success: success_count == total,
            success_count,
            failure_count: total - success_count,
            results,
            batch_id,
            total_execution_time_us: start.elapsed().as_micros() as u64,
        }
    }
}

/// Create the Solver HTTP router with the given handler
///
/// # Example
///
/// ```ignore
/// let handler = MySolverHandler::new();
/// let router = create_router(Arc::new(handler));
/// axum::serve(listener, router).await?;
/// ```
pub fn create_router<H: SolverHttpHandler>(handler: Arc<H>) -> Router {
    Router::new()
        .route("/api/v1/execute-task", post(handle_execute_task::<H>))
        .route("/api/v1/execute-task-batch", post(handle_execute_batch::<H>))
        .route("/health", get(handle_health::<H>))
        .route("/api/v1/info", get(handle_info::<H>))
        .with_state(handler)
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024)) // 10MB limit for batch
}

/// Handle POST /api/v1/execute-task (bincode)
async fn handle_execute_task<H: SolverHttpHandler>(
    State(handler): State<Arc<H>>,
    body: Bytes,
) -> impl IntoResponse {
    let request: ExecuteTaskRequest = match bincode::deserialize(&body) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to deserialize bincode request: {}", e);
            let err_resp = ExecuteTaskResponse::error(
                format!("bincode deserialize error: {}", e),
                0,
            );
            let bytes = bincode::serialize(&err_resp).unwrap_or_default();
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/octet-stream")],
                bytes,
            );
        }
    };

    let task_id_hex = hex::encode(&request.solver_task.task_id[..8]);
    
    info!(
        task_id = %task_id_hex,
        validator_id = %request.validator_id,
        request_id = %request.request_id,
        "Received execute-task request"
    );

    let response = handler.execute_task(request).await;

    if response.success {
        info!(
            task_id = %task_id_hex,
            execution_time_us = response.execution_time_us,
            "Task executed successfully"
        );
    } else {
        error!(
            task_id = %task_id_hex,
            message = %response.message,
            "Task execution failed"
        );
    }

    let bytes = bincode::serialize(&response).unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
}

/// Handle POST /api/v1/execute-task-batch (bincode)
async fn handle_execute_batch<H: SolverHttpHandler>(
    State(handler): State<Arc<H>>,
    body: Bytes,
) -> impl IntoResponse {
    let request: ExecuteBatchRequest = match bincode::deserialize(&body) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to deserialize batch request: {}", e);
            let err_resp = ExecuteBatchResponse {
                results: vec![],
                all_success: false,
                success_count: 0,
                failure_count: 0,
                batch_id: String::new(),
                total_execution_time_us: 0,
            };
            let bytes = bincode::serialize(&err_resp).unwrap_or_default();
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/octet-stream")],
                bytes,
            );
        }
    };

    info!(
        batch_id = %request.batch_id,
        batch_size = request.tasks.len(),
        "Received execute-task-batch request"
    );

    let response = handler.execute_task_batch(request).await;

    info!(
        batch_id = %response.batch_id,
        success = response.success_count,
        failed = response.failure_count,
        time_us = response.total_execution_time_us,
        "Batch execution completed"
    );

    let bytes = bincode::serialize(&response).unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
}

/// Handle GET /health
async fn handle_health<H: SolverHttpHandler>(
    State(handler): State<Arc<H>>,
) -> Json<HealthResponse> {
    Json(handler.health().await)
}

/// Handle GET /api/v1/info
async fn handle_info<H: SolverHttpHandler>(
    State(handler): State<Arc<H>>,
) -> Json<SolverInfoResponse> {
    Json(handler.info().await)
}

/// Start an HTTP server with the given handler
///
/// This is a convenience function that binds to the address and runs the server.
pub async fn start_server<H: SolverHttpHandler>(
    address: &str,
    port: u16,
    handler: Arc<H>,
) -> anyhow::Result<()> {
    let app = create_router(handler);
    let addr = format!("{}:{}", address, port);

    info!(address = %addr, "Starting HTTP server");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Server configuration
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    /// Bind address
    pub address: String,
    /// Port
    pub port: u16,
    /// Enable CORS
    pub enable_cors: bool,
    /// Enable request tracing
    pub enable_tracing: bool,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            address: "0.0.0.0".to_string(),
            port: 8080,
            enable_cors: true,
            enable_tracing: true,
        }
    }
}
