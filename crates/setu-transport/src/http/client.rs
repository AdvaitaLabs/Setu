//! HTTP Client for Validator → Solver communication
//!
//! Provides a typed HTTP client for calling Solver endpoints.

use crate::error::{TransportError, TransportResult};
use crate::http::types::{
    ExecuteTaskRequest, ExecuteTaskResponse, HealthResponse, SolverInfoResponse,
};
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, instrument};

/// HTTP Client for communicating with Solver
///
/// # Example
///
/// ```ignore
/// let client = SolverHttpClient::new("http://solver:8080")?;
/// let response = client.execute_task(request).await?;
/// ```
#[derive(Debug, Clone)]
pub struct SolverHttpClient {
    /// HTTP client instance
    client: Client,
    /// Base URL of the Solver (e.g., "http://solver:8080")
    base_url: String,
}

impl SolverHttpClient {
    /// Create a new Solver HTTP client with default settings
    pub fn new(base_url: impl Into<String>) -> TransportResult<Self> {
        Self::with_config(base_url, SolverHttpClientConfig::default())
    }

    /// Create a new Solver HTTP client with custom configuration
    pub fn with_config(
        base_url: impl Into<String>,
        config: SolverHttpClientConfig,
    ) -> TransportResult<Self> {
        let client = Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .pool_max_idle_per_host(config.max_idle_connections)
            .build()
            .map_err(|e| TransportError::Internal(e.to_string()))?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        })
    }

    /// Execute a SolverTask synchronously
    ///
    /// POST /api/v1/execute-task
    #[instrument(skip(self, request), fields(request_id = %request.request_id))]
    pub async fn execute_task(
        &self,
        request: ExecuteTaskRequest,
    ) -> TransportResult<ExecuteTaskResponse> {
        let url = format!("{}/api/v1/execute-task", self.base_url);

        debug!(url = %url, "Sending execute-task request");

        let response = self.client.post(&url).json(&request).send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Solver returned error");
            return Err(TransportError::ServerError {
                code: status.as_u16(),
                message: body,
            });
        }

        let result: ExecuteTaskResponse = response
            .json()
            .await
            .map_err(|e| TransportError::DeserializationError(e.to_string()))?;

        Ok(result)
    }

    /// Check Solver health
    ///
    /// GET /health
    pub async fn health(&self) -> TransportResult<HealthResponse> {
        let url = format!("{}/health", self.base_url);

        let response = self.client.get(&url).send().await?;

        let status = response.status();
        if !status.is_success() {
            return Err(TransportError::ServerError {
                code: status.as_u16(),
                message: "Health check failed".to_string(),
            });
        }

        let result: HealthResponse = response
            .json()
            .await
            .map_err(|e| TransportError::DeserializationError(e.to_string()))?;

        Ok(result)
    }

    /// Get Solver info
    ///
    /// GET /api/v1/info
    pub async fn info(&self) -> TransportResult<SolverInfoResponse> {
        let url = format!("{}/api/v1/info", self.base_url);

        let response = self.client.get(&url).send().await?;

        let status = response.status();
        if !status.is_success() {
            return Err(TransportError::ServerError {
                code: status.as_u16(),
                message: "Info request failed".to_string(),
            });
        }

        let result: SolverInfoResponse = response
            .json()
            .await
            .map_err(|e| TransportError::DeserializationError(e.to_string()))?;

        Ok(result)
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// Configuration for SolverHttpClient
#[derive(Debug, Clone)]
pub struct SolverHttpClientConfig {
    /// Request timeout
    pub timeout: Duration,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Maximum idle connections per host
    pub max_idle_connections: usize,
}

impl Default for SolverHttpClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            max_idle_connections: 10,
        }
    }
}

impl SolverHttpClientConfig {
    /// Create config with custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Create config with custom connect timeout
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_url_normalization() {
        let client = SolverHttpClient::new("http://localhost:8080/").unwrap();
        assert_eq!(client.base_url(), "http://localhost:8080");

        let client = SolverHttpClient::new("http://localhost:8080").unwrap();
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[test]
    fn test_config_defaults() {
        let config = SolverHttpClientConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
    }
}
