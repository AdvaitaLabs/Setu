//! Registration handler implementation
//!
//! Implements RegistrationHandler trait for Validator RPC.

use super::service::ValidatorNetworkService;
use super::types::{current_timestamp_millis, current_timestamp_secs, ValidatorInfo};
use setu_rpc::{
    GetNodeStatusRequest, GetNodeStatusResponse, GetSolverListRequest, GetSolverListResponse,
    GetValidatorListRequest, GetValidatorListResponse, HeartbeatRequest, HeartbeatResponse,
    NodeType, RegisterSolverRequest, RegisterSolverResponse, RegisterValidatorRequest,
    RegisterValidatorResponse, RegistrationHandler, SolverListItem, UnregisterRequest,
    UnregisterResponse,
};
use setu_types::{Event, SolverRegistration, ValidatorRegistration};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Registration handler implementation for Validator
pub struct ValidatorRegistrationHandler {
    pub(crate) service: Arc<ValidatorNetworkService>,
}

#[async_trait::async_trait]
impl RegistrationHandler for ValidatorRegistrationHandler {
    async fn register_solver(&self, request: RegisterSolverRequest) -> RegisterSolverResponse {
        info!(
            solver_id = %request.solver_id,
            address = %request.address,
            port = request.port,
            account_address = %request.account_address,
            capacity = request.capacity,
            shard_id = ?request.shard_id,
            "Processing solver registration"
        );

        // Check if already registered
        if self
            .service
            .router_manager()
            .get_solver(&request.solver_id)
            .is_some()
        {
            warn!(solver_id = %request.solver_id, "Solver already registered, will update");
        }

        // Create channel and register
        let _channel = self.service.register_solver_internal(&request);

        // Create registration event
        let vlc_time = self.service.get_vlc_time();
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(self.service.validator_id());
        let vlc_snapshot = setu_vlc::VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: current_timestamp_millis(),
        };

        let registration = SolverRegistration::new(
            request.solver_id.clone(),
            request.address.clone(),
            request.port,
            request.account_address.clone(),
            request.public_key.clone(),
            request.signature.clone(),
        )
        .with_capacity(request.capacity)
        .with_shard(request.shard_id.clone().unwrap_or_default())
        .with_resources(request.resources.clone());

        let mut event = Event::solver_register(
            registration,
            vec![],
            vlc_snapshot,
            request.solver_id.clone(),
        );

        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("Solver registration executed".to_string()),
            state_changes: vec![setu_types::event::StateChange {
                key: format!("solver:{}", request.solver_id),
                old_value: None,
                new_value: Some(
                    format!("registered:{}:{}", request.address, request.port).into_bytes(),
                ),
            }],
        });

        // Add event to DAG (async to support consensus submission)
        let event_id = event.id.clone();
        self.service.add_event_to_dag(event).await;

        info!(
            solver_id = %request.solver_id,
            event_id = %&event_id[..20.min(event_id.len())],
            total_solvers = self.service.solver_count(),
            "Solver registered successfully"
        );

        RegisterSolverResponse {
            success: true,
            message: "Solver registered successfully".to_string(),
            assigned_id: Some(request.solver_id),
        }
    }

    async fn register_validator(&self, request: RegisterValidatorRequest) -> RegisterValidatorResponse {
        info!(
            validator_id = %request.validator_id,
            address = %request.address,
            port = request.port,
            account_address = %request.account_address,
            stake_amount = request.stake_amount,
            "Processing validator registration"
        );

        // Create registration event
        let vlc_time = self.service.get_vlc_time();
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(self.service.validator_id());
        let vlc_snapshot = setu_vlc::VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: current_timestamp_millis(),
        };

        let registration = ValidatorRegistration::new(
            request.validator_id.clone(),
            request.address.clone(),
            request.port,
            request.account_address.clone(),
            request.public_key.clone(),
            request.signature.clone(),
            request.stake_amount,
        )
        .with_commission_rate(request.commission_rate);

        let mut event = Event::validator_register(
            registration,
            vec![],
            vlc_snapshot,
            request.validator_id.clone(),
        );

        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("Validator registration executed".to_string()),
            state_changes: vec![setu_types::event::StateChange {
                key: format!("validator:{}", request.validator_id),
                old_value: None,
                new_value: Some(
                    format!("registered:{}:{}", request.address, request.port).into_bytes(),
                ),
            }],
        });

        // Add to validators map
        let now = current_timestamp_secs();
        let validator_info = ValidatorInfo {
            validator_id: request.validator_id.clone(),
            address: request.address.clone(),
            port: request.port,
            status: "online".to_string(),
            registered_at: now,
        };
        self.service.add_validator(validator_info);

        // Add event to DAG (async to support consensus submission)
        self.service.add_event_to_dag(event).await;

        info!(
            validator_id = %request.validator_id,
            total_validators = self.service.validator_count(),
            "Validator registered successfully"
        );

        RegisterValidatorResponse {
            success: true,
            message: "Validator registered successfully".to_string(),
        }
    }

    async fn unregister(&self, request: UnregisterRequest) -> UnregisterResponse {
        info!(
            node_id = %request.node_id,
            node_type = %request.node_type,
            "Processing unregister request"
        );

        match request.node_type {
            NodeType::Solver => {
                self.service.unregister_solver(&request.node_id);
                UnregisterResponse {
                    success: true,
                    message: "Solver unregistered successfully".to_string(),
                }
            }
            NodeType::Validator => {
                self.service.unregister_validator(&request.node_id);
                UnregisterResponse {
                    success: true,
                    message: "Validator unregistered successfully".to_string(),
                }
            }
        }
    }

    async fn heartbeat(&self, request: HeartbeatRequest) -> HeartbeatResponse {
        debug!(
            node_id = %request.node_id,
            current_load = ?request.current_load,
            "Processing heartbeat"
        );

        if let Some(load) = request.current_load {
            self.service
                .router_manager()
                .update_solver_load(&request.node_id, load);
        }

        HeartbeatResponse {
            acknowledged: true,
            server_timestamp: current_timestamp_secs(),
        }
    }

    async fn get_solver_list(&self, request: GetSolverListRequest) -> GetSolverListResponse {
        let solvers = self.service.router_manager().get_all_solvers();

        let solver_list: Vec<SolverListItem> = solvers
            .into_iter()
            .filter(|s| {
                if let Some(ref shard_id) = request.shard_id {
                    s.shard_id.as_ref() == Some(shard_id)
                } else {
                    true
                }
            })
            .map(|s| SolverListItem {
                solver_id: s.id,
                address: s.address.split(':').next().unwrap_or("").to_string(),
                port: s
                    .address
                    .split(':')
                    .nth(1)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0),
                account_address: None,  // TODO: 从 SolverInfo 中获取
                capacity: s.capacity,
                current_load: s.current_load,
                status: format!("{:?}", s.status),
                shard_id: s.shard_id,
            })
            .collect();

        GetSolverListResponse {
            solvers: solver_list,
        }
    }

    async fn get_validator_list(&self, _request: GetValidatorListRequest) -> GetValidatorListResponse {
        GetValidatorListResponse {
            validators: self.service.get_validator_list(),
        }
    }

    async fn get_node_status(&self, request: GetNodeStatusRequest) -> GetNodeStatusResponse {
        // Check if it's a solver
        if let Some(solver) = self.service.router_manager().get_solver(&request.node_id) {
            return GetNodeStatusResponse {
                found: true,
                node_id: request.node_id,
                node_type: Some(NodeType::Solver),
                status: Some(format!("{:?}", solver.status)),
                address: Some(solver.address.split(':').next().unwrap_or("").to_string()),
                port: solver
                    .address
                    .split(':')
                    .nth(1)
                    .and_then(|p| p.parse().ok()),
                uptime_seconds: None,
            };
        }

        // Check if it's a validator
        if let Some(uptime) = self.service.get_validator_uptime(&request.node_id) {
            let info = self.service.get_validator_info(&request.node_id);
            return GetNodeStatusResponse {
                found: true,
                node_id: request.node_id,
                node_type: Some(NodeType::Validator),
                status: info.as_ref().map(|v| v.status.clone()),
                address: info.as_ref().map(|v| v.address.clone()),
                port: info.map(|v| v.port),
                uptime_seconds: Some(uptime),
            };
        }

        // Check if it's this validator
        if request.node_id == self.service.validator_id() {
            return GetNodeStatusResponse {
                found: true,
                node_id: request.node_id,
                node_type: Some(NodeType::Validator),
                status: Some("online".to_string()),
                address: None,
                port: None,
                uptime_seconds: Some(current_timestamp_secs() - self.service.start_time()),
            };
        }

        GetNodeStatusResponse {
            found: false,
            node_id: request.node_id,
            node_type: None,
            status: None,
            address: None,
            port: None,
            uptime_seconds: None,
        }
    }
}
