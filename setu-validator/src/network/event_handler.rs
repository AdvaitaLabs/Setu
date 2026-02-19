//! Event processing and verification logic
//!
//! This module handles:
//! - Event submission and validation
//! - Quick check verification
//! - Sampling verification
//! - DAG management
//! - State queries (Scheme B)

use super::types::*;
use crate::ConsensusValidator;
use dashmap::DashMap;
use parking_lot::RwLock;
use setu_types::event::{Event, EventPayload};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

// Re-export API types
pub use super::types::{
    GetBalanceResponse, GetObjectResponse, SubmitEventRequest, SubmitEventResponse,
};

/// Event handler for processing event submissions and DAG management
pub struct EventHandler;

impl EventHandler {
    /// Process an event submission request
    pub async fn submit_event(
        events: &Arc<DashMap<String, Event>>,
        pending_events: &Arc<RwLock<Vec<String>>>,
        dag_events: &Arc<RwLock<Vec<String>>>,
        validators: &Arc<RwLock<HashMap<String, ValidatorInfo>>>,
        consensus: Option<&Arc<ConsensusValidator>>,
        event_counter: &AtomicU64,
        vlc_counter: &AtomicU64,
        request: SubmitEventRequest,
    ) -> SubmitEventResponse {
        let event = request.event;

        let consensus_enabled = consensus.is_some();
        info!(
            event_id = %&event.id[..20.min(event.id.len())],
            event_type = %event.event_type.name(),
            creator = %event.creator,
            consensus_enabled = consensus_enabled,
            "Receiving event from solver"
        );

        // Quick check
        if let Err(e) = Self::quick_check(&event) {
            return SubmitEventResponse {
                success: false,
                message: format!("Quick check failed: {}", e),
                event_id: Some(event.id),
                vlc_time: None,
            };
        }

        // Add to pending
        pending_events.write().push(event.id.clone());

        // Sampling (10% of events)
        let counter = event_counter.fetch_add(1, Ordering::SeqCst);
        if counter % 10 == 0 {
            if let Err(e) = Self::sampling_verify(&event).await {
                warn!(event_id = %event.id, error = %e, "Sampling verification failed");
            }
        }

        let event_id = event.id.clone();

        // If consensus is enabled, submit to consensus engine
        if let Some(consensus_validator) = consensus {
            match consensus_validator.submit_event(event.clone()).await {
                Ok(_) => {
                    // Get VLC from consensus
                    let vlc = consensus_validator.vlc_snapshot().await;
                    let vlc_time = vlc.logical_time;

                    // Store event locally
                    events.insert(event_id.clone(), event.clone());
                    pending_events.write().retain(|id| id != &event_id);
                    dag_events.write().push(event_id.clone());

                    // Apply side effects
                    Self::apply_event_side_effects(&event, validators);

                    // Log consensus stats
                    let stats = consensus_validator.dag_stats().await;
                    let is_leader = consensus_validator.is_leader().await;
                    info!(
                        event_id = %&event_id[..20.min(event_id.len())],
                        vlc_time = vlc_time,
                        consensus_dag_size = stats.node_count,
                        is_leader = is_leader,
                        "Event added to consensus DAG"
                    );

                    return SubmitEventResponse {
                        success: true,
                        message: "Event verified and added to consensus DAG".to_string(),
                        event_id: Some(event_id),
                        vlc_time: Some(vlc_time),
                    };
                }
                Err(e) => {
                    error!(event_id = %event_id, error = %e, "Failed to submit event to consensus");
                    return SubmitEventResponse {
                        success: false,
                        message: format!("Consensus submission failed: {}", e),
                        event_id: Some(event_id),
                        vlc_time: None,
                    };
                }
            }
        }

        // Legacy path: local VLC and DAG only
        let vlc_time = vlc_counter.fetch_add(1, Ordering::SeqCst) + 1;

        // Store event and add to DAG
        events.insert(event_id.clone(), event.clone());
        pending_events.write().retain(|id| id != &event_id);
        dag_events.write().push(event_id.clone());

        // Apply side effects
        Self::apply_event_side_effects(&event, validators);

        info!(
            event_id = %&event_id[..20.min(event_id.len())],
            vlc_time = vlc_time,
            dag_size = dag_events.read().len(),
            "Event verified (legacy mode)"
        );

        SubmitEventResponse {
            success: true,
            message: "Event verified and added to DAG".to_string(),
            event_id: Some(event_id),
            vlc_time: Some(vlc_time),
        }
    }

    /// Quick check event validity
    fn quick_check(event: &Event) -> Result<(), String> {
        if event.execution_result.is_none() {
            return Err("Event has no execution result".to_string());
        }

        if let Some(ref result) = event.execution_result {
            if !result.success {
                return Err(format!(
                    "Event execution failed: {}",
                    result.message.as_deref().unwrap_or("unknown error")
                ));
            }
        }

        if event.creator.is_empty() {
            return Err("Event creator is empty".to_string());
        }

        let now = current_timestamp_millis();
        if event.timestamp > now + 60000 {
            return Err("Event timestamp is in the future".to_string());
        }

        Ok(())
    }

    /// Sampling verification (simulated)
    async fn sampling_verify(event: &Event) -> Result<(), String> {
        // Simulated: always pass unless "evil" in ID
        if event.id.contains("evil") {
            return Err("Fraud detected".to_string());
        }
        Ok(())
    }

    /// Apply event side effects (e.g., registration updates)
    fn apply_event_side_effects(
        event: &Event,
        validators: &Arc<RwLock<HashMap<String, ValidatorInfo>>>,
    ) {
        match &event.payload {
            EventPayload::ValidatorRegister(reg) => {
                validators.write().insert(
                    reg.validator_id.clone(),
                    ValidatorInfo {
                        validator_id: reg.validator_id.clone(),
                        address: reg.address.clone(),
                        port: reg.port,
                        status: "online".to_string(),
                        registered_at: event.timestamp / 1000,
                    },
                );
            }
            EventPayload::ValidatorUnregister(unreg) => {
                validators.write().remove(&unreg.node_id);
            }
            _ => {} // Other payloads: no side effects needed
        }
    }

    /// Get all events
    pub fn get_events(events: &DashMap<String, Event>) -> Vec<Event> {
        events.iter().map(|e| e.value().clone()).collect()
    }

    // ============================================
    // DAG Management
    // ============================================

    /// Add event to DAG and submit to consensus engine if enabled
    ///
    /// This is the unified entry point for adding events to the DAG.
    /// It ensures events are:
    /// 1. Stored locally for queries
    /// 2. Submitted to consensus engine (if enabled)
    pub async fn add_event_to_dag(
        events: &Arc<DashMap<String, Event>>,
        dag_events: &Arc<RwLock<Vec<String>>>,
        consensus: Option<&Arc<ConsensusValidator>>,
        event: Event,
    ) {
        let event_id = event.id.clone();

        // If consensus is enabled, submit to consensus engine
        if let Some(consensus_validator) = consensus {
            match consensus_validator.submit_event(event.clone()).await {
                Ok(_) => {
                    info!(
                        event_id = %&event_id[..20.min(event_id.len())],
                        "Event submitted to consensus DAG"
                    );
                }
                Err(e) => {
                    error!(
                        event_id = %&event_id[..20.min(event_id.len())],
                        error = %e,
                        "Failed to submit event to consensus, storing locally only"
                    );
                }
            }
        }

        // Always store locally for queries
        events.insert(event_id.clone(), event);
        dag_events.write().push(event_id);
    }

    /// Synchronous version for backward compatibility (legacy mode only)
    ///
    /// WARNING: This does NOT submit to consensus. Use `add_event_to_dag` instead.
    #[allow(dead_code)]
    pub fn add_event_to_dag_sync(
        events: &Arc<DashMap<String, Event>>,
        dag_events: &Arc<RwLock<Vec<String>>>,
        event: Event,
    ) {
        let event_id = event.id.clone();
        events.insert(event_id.clone(), event);
        dag_events.write().push(event_id);
    }

    // ============================================
    // State Query (Scheme B)
    // ============================================

    /// Get account balance (mock implementation)
    pub fn get_balance(account: &str) -> GetBalanceResponse {
        tracing::debug!(account = %account, "Getting balance (mock)");
        GetBalanceResponse {
            account: account.to_string(),
            balance: 1_000_000,
            exists: true,
        }
    }

    /// Get object by key (mock implementation)
    pub fn get_object(key: &str) -> GetObjectResponse {
        tracing::debug!(key = %key, "Getting object (mock)");
        GetObjectResponse {
            key: key.to_string(),
            value: None,
            exists: false,
        }
    }
}
