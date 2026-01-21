//! API-specific types
//!
//! Types used by the HTTP API layer that are not part of the core RPC protocol.

use serde::{Deserialize, Serialize};
use setu_types::event::Event;

// ============================================
// Event Submission
// ============================================

/// Request to submit an event to the validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventRequest {
    /// The event to submit
    pub event: Event,
}

/// Response to event submission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventResponse {
    /// Whether submission was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Event ID
    pub event_id: Option<String>,
    /// VLC time assigned
    pub vlc_time: Option<u64>,
}

// ============================================
// State Query Types (Scheme B)
// ============================================

/// Response for balance query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceResponse {
    /// Account address
    pub account: String,
    /// Balance amount
    pub balance: u128,
    /// Whether the account exists
    pub exists: bool,
}

/// Response for object query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetObjectResponse {
    /// Object key
    pub key: String,
    /// Object value (if exists)
    pub value: Option<Vec<u8>>,
    /// Whether the object exists
    pub exists: bool,
}
