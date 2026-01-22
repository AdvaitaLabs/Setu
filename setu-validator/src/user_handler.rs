//! User RPC Handler Implementation
//!
//! This module implements the UserRpcHandler trait for the Validator,
//! providing user-facing RPC services for wallets and DApps.

use crate::ValidatorNetworkService;
use setu_rpc::{
    UserRpcHandler, RegisterUserRequest, RegisterUserResponse,
    GetAccountRequest, GetAccountResponse, GetBalanceRequest, GetBalanceResponse,
    GetPowerRequest, GetPowerResponse, GetCreditRequest, GetCreditResponse,
    GetCredentialsRequest, GetCredentialsResponse, TransferRequest, TransferResponse,
    ProfileInfo, CoinBalance, PowerChange, CreditChange, CredentialInfo,
    SubmitTransferRequest,
};
use setu_types::event::{Event, EventType};
use setu_types::registration::UserRegistration;
use setu_vlc::VLCSnapshot;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn, error};

/// User RPC Handler for Validator
pub struct ValidatorUserHandler {
    /// Reference to the network service
    network_service: Arc<ValidatorNetworkService>,
}

impl ValidatorUserHandler {
    /// Create a new user handler
    pub fn new(network_service: Arc<ValidatorNetworkService>) -> Self {
        Self { network_service }
    }
}

#[async_trait::async_trait]
impl UserRpcHandler for ValidatorUserHandler {
    async fn register_user(&self, request: RegisterUserRequest) -> RegisterUserResponse {
        info!(
            user_id = %request.user_id,
            subnet_id = ?request.subnet_id,
            invited_by = ?request.invited_by,
            "Processing user registration request"
        );
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              User Registration Flow                        ║");
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Validate request
        info!("[REG 1/5] 🔍 Validating registration request...");
        if request.user_id.is_empty() {
            return RegisterUserResponse {
                success: false,
                message: "User ID cannot be empty".to_string(),
                user_address: None,
                event_id: None,
            };
        }
        
        if request.public_key.is_empty() {
            return RegisterUserResponse {
                success: false,
                message: "Public key cannot be empty".to_string(),
                user_address: None,
                event_id: None,
            };
        }
        
        info!("           └─ Request validation passed");
        
        // Step 2: Generate user address from public key
        info!("[REG 2/5] 🔑 Generating user address...");
        let user_address = format!("0x{}", hex::encode(&request.public_key[..20.min(request.public_key.len())]));
        info!("           └─ User address: {}", user_address);
        
        // Step 3: Create UserRegistration event
        info!("[REG 3/5] 📝 Creating registration event...");
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        let vlc_time = self.network_service.get_vlc_time().await;
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(self.network_service.validator_id());
        let vlc_snapshot = VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: now,
        };
        
        let registration = UserRegistration {
            user_id: request.user_id.clone(),
            public_key: request.public_key.clone(),
            subnet_id: request.subnet_id.clone(),
            display_name: request.display_name.clone(),
            metadata: request.metadata.clone(),
            initial_power: request.initial_power,
            invited_by: request.invited_by.clone(),
            invite_code: request.invite_code.clone(),
        };
        
        let mut event = Event::user_register(
            registration,
            vec![], // No parents for now
            vlc_snapshot,
            self.network_service.validator_id().to_string(),
        );
        
        // Set execution result (simulated successful execution)
        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("User registration executed successfully".to_string()),
            state_changes: vec![
                setu_types::event::StateChange {
                    key: format!("user:{}", request.user_id),
                    old_value: None,
                    new_value: Some(format!("registered:{}", user_address).into_bytes()),
                },
                setu_types::event::StateChange {
                    key: format!("address:{}", user_address),
                    old_value: None,
                    new_value: Some(request.user_id.clone().into_bytes()),
                },
            ],
        });
        
        let event_id = event.id.clone();
        
        info!("           └─ Event ID: {}", &event_id[..20.min(event_id.len())]);
        info!("           └─ VLC Time: {}", vlc_time);
        
        // Step 4: Add event to DAG
        info!("[REG 4/5] 🔗 Adding registration event to DAG...");
        self.network_service.add_event_to_dag(event.clone());
        
        // Note: EventTracker removed in new architecture
        // Event tracking is now handled by the DAG manager
        
        info!("           └─ Event added to DAG");
        
        // Step 5: Apply side effects (update user registry)
        info!("[REG 5/5] 📋 Applying event side effects...");
        self.network_service.apply_event_side_effects(&event_id).await;
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              User Registered Successfully                  ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  User ID:    {:^44} ║", &request.user_id);
        info!("║  Address:    {:^44} ║", &user_address[..20.min(user_address.len())]);
        info!("║  Event ID:   {:^44} ║", &event_id[..20.min(event_id.len())]);
        info!("╚════════════════════════════════════════════════════════════╝");
        
        RegisterUserResponse {
            success: true,
            message: "User registered successfully".to_string(),
            user_address: Some(user_address),
            event_id: Some(event_id),
        }
    }
    
    async fn get_account(&self, request: GetAccountRequest) -> GetAccountResponse {
        info!(address = %request.address, "Getting account information");
        
        // TODO: Query user from storage layer
        // For now, return mock data
        warn!("get_account: Storage integration not implemented, returning mock data");
        
        GetAccountResponse {
            found: true,
            address: request.address.clone(),
            flux_balance: 1000, // TODO: Query from storage
            power: 100,         // TODO: Query from storage
            credit: 50,         // TODO: Query from storage
            profile: Some(ProfileInfo {
                display_name: Some("Mock User".to_string()),
                avatar_url: None,
                bio: None,
                created_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            }),
            credential_count: 0, // TODO: Query from storage
        }
    }
    
    async fn get_balance(&self, request: GetBalanceRequest) -> GetBalanceResponse {
        info!(address = %request.address, "Getting balance");
        
        // TODO: Query balance from storage layer
        warn!("get_balance: Storage integration not implemented, returning mock data");
        
        let balances = vec![
            CoinBalance {
                coin_type: "FLUX".to_string(),
                balance: 1000,
                coin_count: 5,
            },
        ];
        
        let total_balance = balances.iter().map(|b| b.balance).sum();
        
        GetBalanceResponse {
            found: true,
            address: request.address,
            balances,
            total_balance,
        }
    }
    
    async fn get_power(&self, request: GetPowerRequest) -> GetPowerResponse {
        info!(address = %request.address, "Getting power");
        
        // TODO: Query power from storage layer
        warn!("get_power: Storage integration not implemented, returning mock data");
        
        GetPowerResponse {
            found: true,
            address: request.address,
            power: 100,
            rank: Some(42),
            recent_changes: vec![
                PowerChange {
                    amount: 10,
                    reason: "Initial allocation".to_string(),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    event_id: None,
                },
            ],
        }
    }
    
    async fn get_credit(&self, request: GetCreditRequest) -> GetCreditResponse {
        info!(address = %request.address, "Getting credit");
        
        // TODO: Query credit from storage layer
        warn!("get_credit: Storage integration not implemented, returning mock data");
        
        GetCreditResponse {
            found: true,
            address: request.address,
            credit: 50,
            level: Some("Bronze".to_string()),
            recent_changes: vec![
                CreditChange {
                    amount: 5,
                    reason: "Initial credit".to_string(),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    event_id: None,
                },
            ],
        }
    }
    
    async fn get_credentials(&self, request: GetCredentialsRequest) -> GetCredentialsResponse {
        info!(address = %request.address, "Getting credentials");
        
        // TODO: Query credentials from storage layer
        warn!("get_credentials: Storage integration not implemented, returning mock data");
        
        GetCredentialsResponse {
            found: true,
            address: request.address,
            credentials: vec![],
            valid_count: 0,
        }
    }
    
    async fn transfer(&self, request: TransferRequest) -> TransferResponse {
        info!(
            from = %request.from,
            to = %request.to,
            amount = request.amount,
            "Processing transfer request"
        );
        
        // Convert to SubmitTransferRequest
        let submit_request = SubmitTransferRequest {
            from: request.from,
            to: request.to,
            amount: request.amount,
            transfer_type: request.coin_type.unwrap_or_else(|| "flux".to_string()),
            resources: vec![],
            preferred_solver: None,
            shard_id: None,
            subnet_id: None,
        };
        
        // Use existing transfer submission logic
        let response = self.network_service.submit_transfer(submit_request).await;
        
        TransferResponse {
            success: response.success,
            message: response.message,
            event_id: response.transfer_id,
            estimated_confirmation: Some(2), // ~2 seconds
        }
    }
}

