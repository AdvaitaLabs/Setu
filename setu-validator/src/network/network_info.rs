//! Network information endpoints for wallet integration

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Network information response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfoResponse {
    /// Network name
    pub network_name: String,
    
    /// Network version
    pub version: String,
    
    /// Native token symbol
    pub symbol: String,
    
    /// Token decimals
    pub decimals: u8,
    
    /// Total event count (Setu uses DAG, not blocks)
    pub event_count: u64,
    
    /// Network status
    pub status: String,
    
    /// Validator count
    pub validator_count: usize,
    
    /// Solver count
    pub solver_count: usize,
}

/// GET /api/v1/network/info
/// 
/// Get network information for wallet integration
pub async fn get_network_info<S>(
    State(service): State<Arc<S>>,
) -> Result<Json<NetworkInfoResponse>, StatusCode>
where
    S: NetworkInfoProvider,
{
    let info = NetworkInfoResponse {
        network_name: "Setu Network".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        symbol: "FLUX".to_string(),
        decimals: 8,
        event_count: service.dag_events_count() as u64,
        status: "online".to_string(),
        validator_count: service.validator_count(),
        solver_count: service.solver_count(),
    };
    
    Ok(Json(info))
}

/// GET /api/v1/network/chainId
/// 
/// Get chain ID (for compatibility - Setu doesn't use chain ID)
pub async fn get_chain_id() -> Result<Json<ChainIdResponse>, StatusCode> {
    // Setu doesn't have a chain ID concept
    // Return network name as identifier
    Ok(Json(ChainIdResponse {
        chain_id: "setu".to_string(),
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainIdResponse {
    pub chain_id: String,
}

/// Trait for services that can provide network info
pub trait NetworkInfoProvider {
    fn dag_events_count(&self) -> usize;
    fn validator_count(&self) -> usize;
    fn solver_count(&self) -> usize;
}

