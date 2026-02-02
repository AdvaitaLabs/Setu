//! Type definitions for Explorer API

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Common Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    pub page: usize,
    pub limit: usize,
    pub total: usize,
    pub total_pages: usize,
}

// ============================================================================
// Statistics Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub total_anchors: u64,
    pub total_events: u64,
    pub total_validators: usize,
    pub total_solvers: usize,
    pub tps: f64,
    pub avg_anchor_time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestAnchorInfo {
    pub id: String,
    pub depth: u64,
    pub event_count: usize,
    pub timestamp: u64,
    pub vlc_time: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentActivity {
    pub last_24h_events: u64,
    pub last_24h_transfers: u64,
    pub last_24h_registrations: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub network: NetworkStats,
    pub latest_anchor: Option<LatestAnchorInfo>,
    pub recent_activity: RecentActivity,
}

// ============================================================================
// Anchor Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorListItem {
    pub id: String,
    pub depth: u64,
    pub event_count: usize,
    pub timestamp: u64,
    pub vlc_time: u64,
    pub proposer: String,
    pub status: String,
    pub state_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorListResponse {
    pub anchors: Vec<AnchorListItem>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VLCSnapshotInfo {
    pub logical_time: u64,
    pub physical_time: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleRootsInfo {
    pub global_state_root: String,
    pub events_root: String,
    pub anchor_chain_root: String,
    pub subnet_roots: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorStatistics {
    pub transfer_count: usize,
    pub registration_count: usize,
    pub system_event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorDetailResponse {
    pub id: String,
    pub depth: u64,
    pub timestamp: u64,
    pub vlc_snapshot: VLCSnapshotInfo,
    pub previous_anchor: Option<String>,
    pub next_anchor: Option<String>,
    pub event_ids: Vec<String>,
    pub event_count: usize,
    pub merkle_roots: Option<MerkleRootsInfo>,
    pub statistics: AnchorStatistics,
}

// ============================================================================
// Event Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListItem {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub status: String,
    pub creator: String,
    pub timestamp: u64,
    pub vlc_time: u64,
    pub anchor_id: Option<String>,
    pub anchor_depth: Option<u64>,
    pub parent_count: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListResponse {
    pub events: Vec<EventListItem>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResultInfo {
    pub success: bool,
    pub message: String,
    pub state_changes: Vec<StateChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagVisualizationInfo {
    pub depth: u64,
    pub parent_depths: Vec<u64>,
    pub children_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDetailResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub status: String,
    pub creator: String,
    pub timestamp: u64,
    pub vlc_snapshot: VLCSnapshotInfo,
    pub parent_ids: Vec<String>,
    pub children_ids: Vec<String>,
    pub subnet_id: Option<String>,
    pub anchor_id: Option<String>,
    pub anchor_depth: Option<u64>,
    pub payload: serde_json::Value,
    pub execution_result: Option<ExecutionResultInfo>,
    pub dag_visualization: DagVisualizationInfo,
}

// ============================================================================
// DAG Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub id: String,
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub status: String,
    pub depth: u64,
    pub timestamp: u64,
    pub creator: String,
    pub vlc_time: u64,
    pub label: String,
    pub size: usize,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagEdge {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub edge_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMetadata {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub depth_range: (u64, u64),
    pub latest_event_id: String,
    pub anchor_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagLiveResponse {
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagEdge>,
    pub metadata: DagMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalPathResponse {
    pub event_id: String,
    pub ancestors: Vec<DagNode>,
    pub descendants: Vec<DagNode>,
    pub path_edges: Vec<DagEdge>,
}

// ============================================================================
// Search Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(rename = "type")]
    pub result_type: String,
    pub id: String,
    pub url: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_page() -> usize {
    1
}

fn default_limit() -> usize {
    20
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventListParams {
    #[serde(flatten)]
    pub pagination: PaginationParams,
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub status: Option<String>,
    pub creator: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DagLiveParams {
    pub anchor_id: Option<String>,
    pub since_event_id: Option<String>,
    #[serde(default = "default_dag_limit")]
    pub limit: usize,
}

fn default_dag_limit() -> usize {
    100
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchParams {
    pub q: String,
}

