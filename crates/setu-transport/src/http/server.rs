//! HTTP Server abstractions for Solver
//!
//! Provides the server-side framework for handling Validator requests.

use crate::http::types::{
    ExecuteTaskRequest, ExecuteTaskResponse, HealthResponse, SolverInfoResponse,
};
use async_trait::async_trait;
use axum::{
    extract::State,
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
        .route("/health", get(handle_health::<H>))
        .route("/api/v1/info", get(handle_info::<H>))
        .with_state(handler)
}

/// Handle POST /api/v1/execute-task
async fn handle_execute_task<H: SolverHttpHandler>(
    State(handler): State<Arc<H>>,
    Json(request): Json<ExecuteTaskRequest>,
) -> Json<ExecuteTaskResponse> {
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

    Json(response)
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
