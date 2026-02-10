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
    ProfileInfo, CoinBalance, PowerChange, CreditChange,
    SubmitTransferRequest,
};
use setu_types::event::{Event};
use setu_types::registration::UserRegistration;
use setu_vlc::VLCSnapshot;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

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
            address = %request.address,
            subnet_id = ?request.subnet_id,
            invite_code = ?request.invite_code,
            is_metamask = %request.nostr_pubkey.is_none(),
            "Processing user registration request"
        );
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              User Registration Flow                        ║");
        info!("╚════════════════════════════════════════════════════════════╝");
        
        // Step 1: Validate request
        info!("[REG 1/6] 🔍 Validating registration request...");
        if request.address.is_empty() {
            return RegisterUserResponse {
                success: false,
                message: "Wallet address cannot be empty".to_string(),
                address: request.address,
                event_id: None,
                initial_flux: 0,
                initial_power: 0,
                initial_credit: 0,
            };
        }
        
        // Validate address format
        if !request.address.starts_with("0x") || request.address.len() != 42 {
            return RegisterUserResponse {
                success: false,
                message: "Invalid Ethereum address format".to_string(),
                address: request.address,
                event_id: None,
                initial_flux: 0,
                initial_power: 0,
                initial_credit: 0,
            };
        }
        
        // Validate based on registration type
        if let Some(ref nostr_pubkey) = request.nostr_pubkey {
            // Nostr registration
            info!("           └─ Registration type: Nostr");
            if nostr_pubkey.len() != 32 {
                return RegisterUserResponse {
                    success: false,
                    message: "Nostr public key must be 32 bytes".to_string(),
                    address: request.address,
                    event_id: None,
                    initial_flux: 0,
                    initial_power: 0,
                    initial_credit: 0,
                };
            }
            
            if request.signature.is_none() || request.signature.as_ref().unwrap().is_empty() {
                return RegisterUserResponse {
                    success: false,
                    message: "Nostr signature cannot be empty".to_string(),
                    address: request.address,
                    event_id: None,
                    initial_flux: 0,
                    initial_power: 0,
                    initial_credit: 0,
                };
            }
        } else {
            // MetaMask registration
            info!("           └─ Registration type: MetaMask");
            // Signature is optional for MetaMask (middle layer already verified)
            // But if provided, we can do quick_check verification
        }
        
        info!("           └─ Request validation passed");
        
        // Step 2: Verify signature (optional quick_check)
        info!("[REG 2/6] 🔐 Verifying signature...");
        if let Some(ref signature) = request.signature {
            if let Some(ref nostr_pubkey) = request.nostr_pubkey {
                // Nostr signature verification
                info!("           └─ Verifying Nostr Schnorr signature...");
                // TODO: Implement actual Schnorr signature verification
                // Expected: Schnorr signature (64 bytes)
                if signature.len() != 64 {
                    return RegisterUserResponse {
                        success: false,
                        message: "Invalid Nostr signature length (expected 64 bytes)".to_string(),
                        address: request.address,
                        event_id: None,
                        initial_flux: 0,
                        initial_power: 0,
                        initial_credit: 0,
                    };
                }
                info!("           └─ Nostr signature verification passed (mock)");
            } else {
                // MetaMask ECDSA signature verification (optional quick_check)
                info!("           └─ Verifying MetaMask ECDSA signature...");
                // TODO: Implement actual ECDSA signature verification
                // Expected: ECDSA signature (65 bytes: r(32) + s(32) + v(1))
                // Message format: "Register to Setu: {timestamp}"
                if signature.len() != 65 {
                    warn!("           └─ Invalid MetaMask signature length (expected 65 bytes), skipping verification");
                } else {
                    info!("           └─ MetaMask signature verification passed (mock)");
                }
            }
        } else {
            info!("           └─ No signature provided, trusting middle layer verification");
        }
        
        // Step 3: Check if user already registered
        info!("[REG 3/6] 🔍 Checking if user already registered...");
        // TODO: Query storage to check if address exists
        info!("           └─ User not registered yet");
        
        // Step 4: Resolve invite code to inviter address
        let invited_by = if let Some(ref code) = request.invite_code {
            info!("[REG 4/6] 🎫 Resolving invite code: {}", code);
            // TODO: Query storage to resolve invite code to inviter address
            Some(format!("0xinviter_{}", code)) // Mock for now
        } else {
            info!("[REG 4/6] 🎫 No invite code provided");
            None
        };
        
        // Step 5: Create UserRegistration event
        info!("[REG 5/6] 📝 Creating registration event...");
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        let vlc_time = self.network_service.get_vlc_time();
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(self.network_service.validator_id());
        let vlc_snapshot = VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: now,
        };
        
        let registration = UserRegistration {
            address: request.address.clone(),
            nostr_pubkey: request.nostr_pubkey.clone(),
            signature: request.signature.clone(),
            message: request.message.clone(),
            timestamp: request.timestamp,
            subnet_id: request.subnet_id.clone(),
            display_name: request.display_name.clone(),
            metadata: request.metadata.clone(),
            invited_by: invited_by.clone(),
            invite_code: request.invite_code.clone(),
        };
        
        // Initial allocations
        // Flux: Starts at 0, can only be minted by paying USDC or earned through POCW
        let initial_flux = 0u64;
        
        // Power: Fixed initial allocation of 21 million (non-renewable, non-transferable)
        // Power is consumed by any system event participation
        // Note: 1 Flux = 10^32 in actual value representation
        let initial_power = 21_000_000u64;
        
        // Credit: Initial reputation score
        let initial_credit = 50u64;
        
        let mut event = Event::user_register(
            registration,
            vec![], // No parents for now
            vlc_snapshot,
            self.network_service.validator_id().to_string(),
        );
        
        // Create Coin Objects for Flux and Power (Scheme A)
        use setu_types::coin::{Coin, CoinType};
        use setu_types::Address;
        
        // Parse address - Ethereum address is 20 bytes, pad to 32 bytes for Setu Address
        let addr_str = request.address.strip_prefix("0x").unwrap_or(&request.address);
        let mut addr_bytes = [0u8; 32];
        if let Ok(decoded) = hex::decode(addr_str) {
            // Copy the decoded bytes (up to 32 bytes)
            let len = decoded.len().min(32);
            addr_bytes[..len].copy_from_slice(&decoded[..len]);
        }
        let owner_address = Address::new(addr_bytes);
        
        // Create Flux Coin Object (initial balance: 0)
        let flux_coin = Coin::new_with_type(
            owner_address.clone(),
            initial_flux,
            CoinType::new("FLUX"),
        );
        
        // Create Power Coin Object (initial balance: 21 million)
        let power_coin = Coin::new_with_type(
            owner_address.clone(),
            initial_power,
            CoinType::new("POWER"),
        );
        
        // Serialize Coin Objects
        let flux_coin_bytes = bcs::to_bytes(&flux_coin)
            .expect("Failed to serialize Flux coin");
        let power_coin_bytes = bcs::to_bytes(&power_coin)
            .expect("Failed to serialize Power coin");
        
        // Get object IDs for storage keys
        let flux_object_id = hex::encode(flux_coin.id().as_bytes());
        let power_object_id = hex::encode(power_coin.id().as_bytes());
        
        info!("           └─ Created Flux Coin Object: {}", &flux_object_id[..16]);
        info!("           └─ Created Power Coin Object: {}", &power_object_id[..16]);
        
        // Set execution result with Coin Object creation
        event.set_execution_result(setu_types::event::ExecutionResult {
            success: true,
            message: Some("User registration executed successfully with Coin Objects".to_string()),
            state_changes: vec![
                // User registration marker
                setu_types::event::StateChange {
                    key: format!("user:{}", request.address),
                    old_value: None,
                    new_value: Some(format!("registered").into_bytes()),
                },
                // Create Flux Coin Object
                setu_types::event::StateChange {
                    key: format!("object:{}", flux_object_id),
                    old_value: None,
                    new_value: Some(flux_coin_bytes),
                },
                // Create Power Coin Object
                setu_types::event::StateChange {
                    key: format!("object:{}", power_object_id),
                    old_value: None,
                    new_value: Some(power_coin_bytes),
                },
                // Credit score (not a Coin Object, just a value)
                setu_types::event::StateChange {
                    key: format!("credit:{}", request.address),
                    old_value: None,
                    new_value: Some(initial_credit.to_string().into_bytes()),
                },
            ],
        });
        
        let event_id = event.id.clone();
        
        info!("           └─ Event ID: {}", &event_id[..20.min(event_id.len())]);
        info!("           └─ VLC Time: {}", vlc_time);
        
        // Step 6: Add event to DAG (async to support consensus submission)
        info!("[REG 6/6] 🔗 Adding registration event to DAG...");
        self.network_service.add_event_to_dag(event.clone()).await;
        
        info!("           └─ Event added to DAG");
        
        // Apply side effects (update user registry)
        self.network_service.apply_event_side_effects(&event_id).await;
        
        info!("╔════════════════════════════════════════════════════════════╗");
        info!("║              User Registered Successfully                  ║");
        info!("╠════════════════════════════════════════════════════════════╣");
        info!("║  Address:    {:^44} ║", &request.address[..20.min(request.address.len())]);
        info!("║  Event ID:   {:^44} ║", &event_id[..20.min(event_id.len())]);
        info!("║  Flux:       {:^44} ║", initial_flux);
        info!("║  Power:      {:^44} ║", initial_power);
        info!("║  Credit:     {:^44} ║", initial_credit);
        info!("╚════════════════════════════════════════════════════════════╝");
        
        RegisterUserResponse {
            success: true,
            message: "User registered successfully".to_string(),
            address: request.address,
            event_id: Some(event_id),
            initial_flux,
            initial_power,
            initial_credit,
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

