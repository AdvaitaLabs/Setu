//! Registration handler implementation
//!
//! Implements RegistrationHandler trait for Validator RPC.

use super::service::ValidatorNetworkService;
use super::types::{current_timestamp_millis, current_timestamp_secs, ValidatorInfo, SubnetInfo};
use setu_rpc::{
    GetNodeStatusRequest, GetNodeStatusResponse, GetSolverListRequest, GetSolverListResponse,
    GetValidatorListRequest, GetValidatorListResponse, HeartbeatRequest, HeartbeatResponse,
    GetSubnetListRequest, GetSubnetListResponse,
    NodeType, RegisterSolverRequest, RegisterSolverResponse, RegisterValidatorRequest,
    RegisterValidatorResponse, RegisterSubnetRequest, RegisterSubnetResponse,
    RegistrationHandler, SolverListItem, UnregisterRequest,
    UnregisterResponse,
};
use setu_types::{Event, SolverRegistration, ValidatorRegistration};
use setu_types::registration::{SubnetRegistration, SubnetResourceLimits, TokenConfig};
use setu_types::subnet::SubnetType;
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

        // Linkage: also update consensus layer (ValidatorSet + validator_count)
        if let Some(cv) = self.service.consensus_validator() {
            let peer_node_info = setu_types::NodeInfo::new_validator(
                request.validator_id.clone(),
                request.address.clone(),
                request.port,
            );
            cv.add_peer_validator(peer_node_info).await;
            info!(
                "Consensus layer updated: validator {} added",
                request.validator_id
            );
        }

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

    async fn register_subnet(&self, request: RegisterSubnetRequest) -> RegisterSubnetResponse {
        info!(
            subnet_id = %request.subnet_id,
            name = %request.name,
            owner = %request.owner,
            token_symbol = %request.token_symbol,
            "Processing subnet registration"
        );

        // Check if already registered
        if self.service.get_subnet_info(&request.subnet_id).is_some() {
            warn!(subnet_id = %request.subnet_id, "Subnet already registered");
            return RegisterSubnetResponse {
                success: false,
                message: format!("Subnet '{}' is already registered", request.subnet_id),
                subnet_id: Some(request.subnet_id),
                event_id: None,
            };
        }

        // Validate owner address (must be 0x + 64 hex chars = 66 total)
        if !request.owner.starts_with("0x")
            || request.owner.len() != 66
            || !request.owner[2..].chars().all(|c| c.is_ascii_hexdigit())
        {
            return RegisterSubnetResponse {
                success: false,
                message: "Invalid owner address format (expected 0x + 64 hex chars)".to_string(),
                subnet_id: None,
                event_id: None,
            };
        }

        // Validate token_symbol: 1-10 uppercase alphanumeric characters
        if request.token_symbol.is_empty()
            || request.token_symbol.len() > 10
            || !request.token_symbol.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return RegisterSubnetResponse {
                success: false,
                message: "Invalid token_symbol (must be 1-10 uppercase alphanumeric characters)".to_string(),
                subnet_id: None,
                event_id: None,
            };
        }

        // Parse subnet type
        let subnet_type = match request.subnet_type.as_deref() {
            None => SubnetType::App,
            Some(t) => match t.to_ascii_lowercase().as_str() {
                "app" | "application" => SubnetType::App,
                "organization" | "org" => SubnetType::Organization,
                "personal" => SubnetType::Personal,
                other => {
                    return RegisterSubnetResponse {
                        success: false,
                        message: format!("Unknown subnet type '{}' (valid: app, organization, personal)", other),
                        subnet_id: None,
                        event_id: None,
                    };
                }
            },
        };

        // Build resource limits
        let resource_limits = if request.max_tps.is_some() || request.max_storage_bytes.is_some() {
            let mut limits = SubnetResourceLimits::new();
            if let Some(tps) = request.max_tps {
                limits = limits.with_tps(tps);
            }
            if let Some(storage) = request.max_storage_bytes {
                limits = limits.with_storage(storage);
            }
            Some(limits)
        } else {
            None
        };

        // Build token config
        let token_config = TokenConfig {
            decimals: request.token_decimals.unwrap_or(8),
            max_supply: request.token_max_supply,
            mintable: request.token_mintable.unwrap_or(false),
            burnable: request.token_burnable.unwrap_or(true),
        };

        // Build SubnetRegistration
        let mut registration = SubnetRegistration::new(
            request.subnet_id.clone(),
            request.name.clone(),
            request.owner.clone(),
            request.token_symbol.clone(),
        )
        .with_type(subnet_type)
        .with_token_config(token_config);

        if let Some(desc) = &request.description {
            registration = registration.with_description(desc.clone());
        }
        if let Some(parent) = &request.parent_subnet_id {
            registration = registration.with_parent(parent.clone());
        }
        if let Some(max_users) = request.max_users {
            registration = registration.with_max_users(max_users);
        }
        if let Some(limits) = resource_limits {
            registration = registration.with_limits(limits);
        }
        if let Some(supply) = request.initial_token_supply {
            registration = registration.with_initial_supply(supply);
        }
        if let Some(airdrop) = request.user_airdrop_amount {
            registration = registration.with_user_airdrop(airdrop);
        }
        if !request.assigned_solvers.is_empty() {
            registration = registration.with_solvers(request.assigned_solvers.clone());
        }

        // Create VLC snapshot
        let vlc_time = self.service.get_vlc_time();
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(self.service.validator_id());
        let vlc_snapshot = setu_vlc::VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: current_timestamp_millis(),
        };

        // Create subnet registration event
        let mut event = Event::subnet_register(
            registration.clone(),
            vec![],
            vlc_snapshot,
            self.service.validator_id().to_string(),
        );

        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("Subnet registration executed".to_string()),
            state_changes: vec![setu_types::event::StateChange {
                key: format!("subnet:{}", request.subnet_id),
                old_value: None,
                new_value: Some(
                    serde_json::json!({
                        "name": request.name,
                        "owner": request.owner,
                        "token_symbol": request.token_symbol,
                        "subnet_type": format!("{:?}", registration.subnet_type),
                    }).to_string().into_bytes(),
                ),
            }],
        });

        let event_id = event.id.clone();

        // Track subnet locally
        self.service.add_subnet(SubnetInfo {
            subnet_id: request.subnet_id.clone(),
            name: request.name.clone(),
            owner: request.owner.clone(),
            subnet_type: format!("{:?}", registration.subnet_type),
            token_symbol: request.token_symbol.clone(),
            status: "active".to_string(),
            registered_at: event.timestamp / 1000,
        });

        // Add event to DAG
        self.service.add_event_to_dag(event).await;

        info!(
            subnet_id = %request.subnet_id,
            event_id = %&event_id[..20.min(event_id.len())],
            "Subnet registered successfully"
        );

        RegisterSubnetResponse {
            success: true,
            message: "Subnet registered successfully".to_string(),
            subnet_id: Some(request.subnet_id),
            event_id: Some(event_id),
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

    async fn get_subnet_list(&self, request: GetSubnetListRequest) -> GetSubnetListResponse {
        let mut subnets = self.service.get_subnet_list();

        if let Some(ref type_filter) = request.type_filter {
            subnets.retain(|s| s.subnet_type.eq_ignore_ascii_case(type_filter));
        }
        if let Some(ref owner_filter) = request.owner_filter {
            subnets.retain(|s| s.owner == *owner_filter);
        }

        GetSubnetListResponse { subnets }
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
