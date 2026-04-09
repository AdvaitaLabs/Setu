//! User RPC Handler Implementation
//!
//! This module implements the UserRpcHandler trait for the Validator,
//! providing user-facing RPC services for wallets and DApps.
//!
//! Registration delegates to InfraExecutor for G11-compliant state changes.
//! Balance/account queries read from MerkleStateProvider (StateProvider trait).

use crate::ValidatorNetworkService;
use setu_rpc::{
    UserRpcHandler, RegisterUserRequest, RegisterUserResponse,
    GetAccountRequest, GetAccountResponse, GetBalanceRequest, GetBalanceResponse,
    GetPowerRequest, GetPowerResponse, GetFluxRequest, GetFluxResponse,
    GetCredentialsRequest, GetCredentialsResponse, TransferRequest, TransferResponse,
    CoinBalance, SubmitTransferRequest,
    // Phase 3
    UpdateProfileRequest, UpdateProfileResponse,
    GetProfileResponse, ProfileInfo,
    JoinSubnetRequest, JoinSubnetResponse,
    LeaveSubnetRequest, LeaveSubnetResponse,
    CheckMembershipResponse, GetUserSubnetsResponse,
};
use setu_types::registration::UserRegistration;
use setu_types::{ObjectId, hash_utils::setu_hash_with_domain};
use setu_types::{FluxState, PowerState, flux_state_object_id, power_state_object_id, INITIAL_POWER, INITIAL_FLUX};
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

    /// Build error response for register_user
    fn reg_err(message: &str, address: &str) -> RegisterUserResponse {
        RegisterUserResponse {
            success: false,
            message: message.to_string(),
            address: address.to_string(),
            event_id: None,
            initial_setu: 0,
            initial_power: 0,
            initial_flux: 0,
        }
    }

    /// Verify signature for a write operation (3-branch: MetaMask / Setu native / Nostr).
    /// Returns Ok(()) if valid, or error message string if invalid.
    fn verify_signature(
        address: &str,
        signature: &[u8],
        message: &str,
        nostr_pubkey: Option<&[u8]>,
        public_key: Option<&str>,
    ) -> Result<(), String> {
        if std::env::var("SETU_SKIP_SIG_VERIFY").unwrap_or_default() == "1" {
            return Ok(());
        }
        let result = if let Some(npk) = nostr_pubkey {
            setu_keys::verify::verify_nostr_schnorr(address, npk, signature, message.as_bytes())
        } else if let Some(pk_b64) = public_key {
            let pk_raw = setu_keys::PublicKey::decode_base64(pk_b64)
                .and_then(|pk| {
                    let mut v = vec![pk.scheme().flag()];
                    v.extend(pk.as_bytes());
                    Ok(v)
                });
            match pk_raw {
                Ok(pk_bytes) => setu_keys::verify::verify_setu_native_raw(
                    address, &pk_bytes, signature, message.as_bytes(),
                ),
                Err(e) => Err(e),
            }
        } else {
            setu_keys::verify::verify_metamask_personal_sign(address, signature, message)
        };
        result.map_err(|e| format!("Signature verification failed: {}", e))
    }

    /// Build VLC snapshot for a new event
    fn build_vlc_snapshot(&self) -> VLCSnapshot {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let vlc_time = self.network_service.get_vlc_time();
        let mut vlc = setu_vlc::VectorClock::new();
        vlc.increment(self.network_service.validator_id());
        VLCSnapshot {
            vector_clock: vlc,
            logical_time: vlc_time,
            physical_time: now,
        }
    }

    /// Validate timestamp is within 5-minute anti-replay window
    fn check_timestamp(timestamp: u64) -> Result<(), String> {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req_secs = timestamp / 1000;
        if now_secs.abs_diff(req_secs) > 300 {
            return Err("Timestamp too old or too far in the future (5 min window)".to_string());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl UserRpcHandler for ValidatorUserHandler {
    async fn register_user(&self, request: RegisterUserRequest) -> RegisterUserResponse {
        info!(
            address = %request.address,
            subnet_id = ?request.subnet_id,
            is_metamask = %request.nostr_pubkey.is_none(),
            "Processing user registration request"
        );

        // ── Step 1: Validate request ────────────────────────────────
        if request.address.is_empty() {
            return Self::reg_err("Wallet address cannot be empty", &request.address);
        }

        // Accept 66-char Setu native (0x + 64 hex) or 42-char Ethereum (0x + 40 hex)
        if !request.address.starts_with("0x")
            || (request.address.len() != 66 && request.address.len() != 42)
        {
            return Self::reg_err(
                "Invalid address format: expected 0x + 64 hex (Setu) or 0x + 40 hex (Ethereum)",
                &request.address,
            );
        }

        // Nostr-specific validation
        if let Some(ref nostr_pubkey) = request.nostr_pubkey {
            if nostr_pubkey.len() != 32 {
                return Self::reg_err("Nostr public key must be 32 bytes", &request.address);
            }
            if request.signature.is_none() || request.signature.as_ref().unwrap().is_empty() {
                return Self::reg_err("Nostr signature cannot be empty", &request.address);
            }
        }

        // ── Step 2: Timestamp anti-replay ──────────────────────────
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req_secs = request.timestamp / 1000; // request.timestamp is millis
        if now_secs.abs_diff(req_secs) > 300 {
            return Self::reg_err(
                "Timestamp too old or too far in the future (5 min window)",
                &request.address,
            );
        }

        // ── Step 3: Signature verification ──────────────────────────
        let skip_sig = std::env::var("SETU_SKIP_SIG_VERIFY").unwrap_or_default() == "1";

        if !skip_sig {
            let message = match &request.message {
                Some(m) => m.clone(),
                None => {
                    return Self::reg_err(
                        "Signed message is required for registration",
                        &request.address,
                    );
                }
            };

            let signature = match &request.signature {
                Some(s) if !s.is_empty() => s,
                _ => {
                    return Self::reg_err(
                        "Signature is required for registration",
                        &request.address,
                    );
                }
            };

            let sig_result = if let Some(ref nostr_pubkey) = request.nostr_pubkey {
                // Nostr: Schnorr BIP-340
                setu_keys::verify::verify_nostr_schnorr(
                    &request.address,
                    nostr_pubkey,
                    signature,
                    message.as_bytes(),
                )
            } else if let Some(ref public_key_b64) = request.public_key {
                // Setu native: Ed25519 / Secp256k1 / Secp256r1
                // public_key is base64 (flag || pk_bytes), signature is raw bytes.
                let pk_raw = setu_keys::PublicKey::decode_base64(public_key_b64)
                    .and_then(|pk| {
                        let mut v = vec![pk.scheme().flag()];
                        v.extend(pk.as_bytes());
                        Ok(v)
                    });
                match pk_raw {
                    Ok(pk_bytes) => setu_keys::verify::verify_setu_native_raw(
                        &request.address,
                        &pk_bytes,
                        signature,
                        message.as_bytes(),
                    ),
                    Err(e) => Err(e),
                }
            } else {
                // MetaMask: secp256k1 ECDSA with personal_sign recovery
                setu_keys::verify::verify_metamask_personal_sign(
                    &request.address,
                    signature,
                    &message,
                )
            };

            if let Err(e) = sig_result {
                warn!(address = %request.address, error = %e, "Signature verification failed");
                return Self::reg_err(
                    &format!("Signature verification failed: {}", e),
                    &request.address,
                );
            }
        }

        // ── Step 4: Duplicate registration detection ────────────────
        let subnet_id = request.subnet_id.as_deref().unwrap_or("subnet-0");
        let membership_key = format!("user:{}:subnet:{}", request.address, subnet_id);
        let membership_object_id = ObjectId::new(
            setu_hash_with_domain(b"SETU_MEMBERSHIP:", membership_key.as_bytes()),
        );

        if self
            .network_service
            .state_provider()
            .get_object(&membership_object_id)
            .is_some()
        {
            return Self::reg_err(
                &format!(
                    "User {} already registered in subnet '{}'",
                    request.address, subnet_id
                ),
                &request.address,
            );
        }

        // ── Step 5: Build VLC snapshot ──────────────────────────────
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

        // ── Step 6: Build UserRegistration ──────────────────────────
        let registration = UserRegistration {
            address: request.address.clone(),
            nostr_pubkey: request.nostr_pubkey.clone(),
            signature: request.signature.clone(),
            message: request.message.clone(),
            timestamp: request.timestamp,
            subnet_id: request.subnet_id.clone(),
            display_name: request.display_name.clone(),
            metadata: request.metadata.clone(),
            invited_by: None,
            invite_code: request.invite_code.clone(),
            public_key: request.public_key.clone(),
        };

        // ── Step 7: Delegate to InfraExecutor (路径 B) ──────────────
        // InfraExecutor:
        //   → RuntimeExecutor::execute_user_register()  (G11-compliant "oid:{hex}" state keys)
        //   → apply_state_changes() to MerkleStateProvider
        //   → returns Event with execution_result set
        let event = match self
            .network_service
            .infra_executor()
            .execute_user_register(&registration, vlc_snapshot)
        {
            Ok(event) => event,
            Err(e) => {
                error!(address = %request.address, error = %e, "InfraExecutor user registration failed");
                return Self::reg_err(&format!("Registration failed: {}", e), &request.address);
            }
        };

        let event_id = event.id.clone();

        // ── Step 8: Add event to DAG ────────────────────────────────
        self.network_service.add_event_to_dag(event).await;

        info!(
            address = %request.address,
            event_id = %event_id,
            "User registered successfully (zero initial balance — use Faucet for tokens)"
        );

        RegisterUserResponse {
            success: true,
            message: "User registered successfully".to_string(),
            address: request.address,
            event_id: Some(event_id),
            initial_setu: 0,
            initial_power: INITIAL_POWER,
            initial_flux: INITIAL_FLUX,
        }
    }
    
    async fn get_account(&self, request: GetAccountRequest) -> GetAccountResponse {
        info!(address = %request.address, "Getting account information");

        let coins = self.network_service.state_provider().get_coins_for_address(&request.address);
        let setu_balance: u64 = coins.iter()
            .filter(|c| c.coin_type == "ROOT")
            .map(|c| c.balance)
            .sum();

        // Read Power from Merkle tree
        let power_oid = power_state_object_id(&request.address);
        let power = self.network_service.state_provider()
            .get_object(&power_oid)
            .and_then(|bytes| serde_json::from_slice::<PowerState>(&bytes).ok())
            .map(|ps| ps.power_remaining)
            .unwrap_or(0);

        // Read Flux from Merkle tree
        let flux_oid = flux_state_object_id(&request.address);
        let flux = self.network_service.state_provider()
            .get_object(&flux_oid)
            .and_then(|bytes| serde_json::from_slice::<FluxState>(&bytes).ok())
            .map(|fs| fs.flux)
            .unwrap_or(0);

        GetAccountResponse {
            found: !coins.is_empty(),
            address: request.address,
            setu_balance,
            power,
            flux,
            profile: None,
            credential_count: 0,
        }
    }
    
    async fn get_balance(&self, request: GetBalanceRequest) -> GetBalanceResponse {
        info!(address = %request.address, "Getting balance");

        let coins = self.network_service.state_provider().get_coins_for_address(&request.address);

        // Aggregate by coin_type
        let mut type_map: std::collections::HashMap<String, (u64, u32)> = std::collections::HashMap::new();
        for c in &coins {
            let entry = type_map.entry(c.coin_type.clone()).or_insert((0, 0));
            entry.0 += c.balance;
            entry.1 += 1;
        }

        // Optional filter by coin_type
        let balances: Vec<CoinBalance> = type_map.into_iter()
            .filter(|(ct, _)| {
                request.coin_type.as_ref().map_or(true, |filter| ct == filter)
            })
            .map(|(coin_type, (balance, coin_count))| CoinBalance {
                coin_type,
                balance,
                coin_count,
            })
            .collect();

        let total_balance = balances.iter().map(|b| b.balance).sum();

        GetBalanceResponse {
            found: !coins.is_empty(),
            address: request.address,
            balances,
            total_balance,
        }
    }
    
    async fn get_power(&self, request: GetPowerRequest) -> GetPowerResponse {
        let power_oid = power_state_object_id(&request.address);
        match self.network_service.state_provider()
            .get_object(&power_oid)
            .and_then(|bytes| serde_json::from_slice::<PowerState>(&bytes).ok())
        {
            Some(ps) => GetPowerResponse {
                found: true,
                address: request.address,
                power: ps.power_remaining,
                rank: None,
                recent_changes: vec![],
            },
            None => GetPowerResponse {
                found: false,
                address: request.address,
                power: 0,
                rank: None,
                recent_changes: vec![],
            },
        }
    }
    
    async fn get_flux(&self, request: GetFluxRequest) -> GetFluxResponse {
        let flux_oid = flux_state_object_id(&request.address);
        match self.network_service.state_provider()
            .get_object(&flux_oid)
            .and_then(|bytes| serde_json::from_slice::<FluxState>(&bytes).ok())
        {
            Some(fs) => GetFluxResponse {
                found: true,
                address: request.address,
                flux: fs.flux,
                level: None,
                recent_changes: vec![],
            },
            None => GetFluxResponse {
                found: false,
                address: request.address,
                flux: 0,
                level: None,
                recent_changes: vec![],
            },
        }
    }
    
    async fn get_credentials(&self, request: GetCredentialsRequest) -> GetCredentialsResponse {
        // Credential system not yet implemented — return empty
        GetCredentialsResponse {
            found: false,
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
            transfer_type: request.coin_type.unwrap_or_else(|| "setu".to_string()),
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

    // ========== Phase 3: Profile & Subnet Membership ==========

    async fn update_profile(&self, request: UpdateProfileRequest) -> UpdateProfileResponse {
        info!(address = %request.address, "Processing profile update");

        // Validate address format
        if !request.address.starts_with("0x")
            || (request.address.len() != 66 && request.address.len() != 42)
        {
            return UpdateProfileResponse {
                success: false,
                message: "Invalid address format".to_string(),
                event_id: None,
            };
        }

        // Timestamp anti-replay
        if let Err(e) = Self::check_timestamp(request.timestamp) {
            return UpdateProfileResponse { success: false, message: e, event_id: None };
        }

        // Signature verification
        if let Err(e) = Self::verify_signature(
            &request.address, &request.signature, &request.message,
            request.nostr_pubkey.as_deref(), request.public_key.as_deref(),
        ) {
            warn!(address = %request.address, error = %e, "Profile update sig failed");
            return UpdateProfileResponse { success: false, message: e, event_id: None };
        }

        let vlc_snapshot = self.build_vlc_snapshot();
        let attrs = request.attributes.unwrap_or_default();

        let event = match self.network_service.infra_executor().execute_profile_update(
            &request.address,
            request.display_name.as_deref(),
            request.avatar_url.as_deref(),
            request.bio.as_deref(),
            &attrs,
            vlc_snapshot,
        ) {
            Ok(event) => event,
            Err(e) => {
                error!(address = %request.address, error = %e, "Profile update failed");
                return UpdateProfileResponse {
                    success: false, message: format!("Profile update failed: {}", e), event_id: None,
                };
            }
        };

        let event_id = event.id.clone();
        self.network_service.add_event_to_dag(event).await;

        info!(address = %request.address, event_id = %event_id, "Profile updated");
        UpdateProfileResponse { success: true, message: "Profile updated".to_string(), event_id: Some(event_id) }
    }

    async fn get_profile(&self, address: &str) -> GetProfileResponse {
        let profile_key = format!("profile:{}", address);
        let profile_object_id = ObjectId::new(
            setu_hash_with_domain(b"SETU_PROFILE:", profile_key.as_bytes()),
        );

        match self.network_service.state_provider().get_object(&profile_object_id) {
            Some(data) => {
                let profile: serde_json::Value = serde_json::from_slice(&data).unwrap_or_default();
                GetProfileResponse {
                    found: true,
                    address: address.to_string(),
                    profile: Some(ProfileInfo {
                        display_name: profile["display_name"].as_str().map(|s| s.to_string()),
                        avatar_url: profile["avatar_url"].as_str().map(|s| s.to_string()),
                        bio: profile["bio"].as_str().map(|s| s.to_string()),
                        created_at: profile["created_at"].as_u64().unwrap_or(0),
                    }),
                }
            }
            None => GetProfileResponse {
                found: false,
                address: address.to_string(),
                profile: None,
            },
        }
    }

    async fn join_subnet(&self, request: JoinSubnetRequest) -> JoinSubnetResponse {
        info!(address = %request.address, subnet_id = %request.subnet_id, "Processing subnet join");

        if !request.address.starts_with("0x")
            || (request.address.len() != 66 && request.address.len() != 42)
        {
            return JoinSubnetResponse {
                success: false, message: "Invalid address format".to_string(), event_id: None,
            };
        }

        if let Err(e) = Self::check_timestamp(request.timestamp) {
            return JoinSubnetResponse { success: false, message: e, event_id: None };
        }

        if let Err(e) = Self::verify_signature(
            &request.address, &request.signature, &request.message,
            request.nostr_pubkey.as_deref(), request.public_key.as_deref(),
        ) {
            warn!(address = %request.address, error = %e, "Subnet join sig failed");
            return JoinSubnetResponse { success: false, message: e, event_id: None };
        }

        // Duplicate join detection
        let membership_key = format!("user:{}:subnet:{}", request.address, request.subnet_id);
        let membership_oid = ObjectId::new(
            setu_hash_with_domain(b"SETU_MEMBERSHIP:", membership_key.as_bytes()),
        );
        if self.network_service.state_provider().get_object(&membership_oid).is_some() {
            return JoinSubnetResponse {
                success: false,
                message: format!("User {} already a member of subnet '{}'", request.address, request.subnet_id),
                event_id: None,
            };
        }

        let vlc_snapshot = self.build_vlc_snapshot();
        let event = match self.network_service.infra_executor().execute_subnet_join(
            &request.address, &request.subnet_id, vlc_snapshot,
        ) {
            Ok(event) => event,
            Err(e) => {
                error!(address = %request.address, error = %e, "Subnet join failed");
                return JoinSubnetResponse {
                    success: false, message: format!("Subnet join failed: {}", e), event_id: None,
                };
            }
        };

        let event_id = event.id.clone();
        self.network_service.add_event_to_dag(event).await;

        info!(address = %request.address, subnet_id = %request.subnet_id, event_id = %event_id, "Joined subnet");
        JoinSubnetResponse { success: true, message: "Joined subnet".to_string(), event_id: Some(event_id) }
    }

    async fn leave_subnet(&self, request: LeaveSubnetRequest) -> LeaveSubnetResponse {
        info!(address = %request.address, subnet_id = %request.subnet_id, "Processing subnet leave");

        if !request.address.starts_with("0x")
            || (request.address.len() != 66 && request.address.len() != 42)
        {
            return LeaveSubnetResponse {
                success: false, message: "Invalid address format".to_string(), event_id: None,
            };
        }

        if let Err(e) = Self::check_timestamp(request.timestamp) {
            return LeaveSubnetResponse { success: false, message: e, event_id: None };
        }

        if let Err(e) = Self::verify_signature(
            &request.address, &request.signature, &request.message,
            request.nostr_pubkey.as_deref(), request.public_key.as_deref(),
        ) {
            warn!(address = %request.address, error = %e, "Subnet leave sig failed");
            return LeaveSubnetResponse { success: false, message: e, event_id: None };
        }

        // Existence check: must be a member to leave
        let membership_key = format!("user:{}:subnet:{}", request.address, request.subnet_id);
        let membership_oid = ObjectId::new(
            setu_hash_with_domain(b"SETU_MEMBERSHIP:", membership_key.as_bytes()),
        );
        if self.network_service.state_provider().get_object(&membership_oid).is_none() {
            return LeaveSubnetResponse {
                success: false,
                message: format!("User {} is not a member of subnet '{}'", request.address, request.subnet_id),
                event_id: None,
            };
        }

        let vlc_snapshot = self.build_vlc_snapshot();
        let event = match self.network_service.infra_executor().execute_subnet_leave(
            &request.address, &request.subnet_id, vlc_snapshot,
        ) {
            Ok(event) => event,
            Err(e) => {
                error!(address = %request.address, error = %e, "Subnet leave failed");
                return LeaveSubnetResponse {
                    success: false, message: format!("Subnet leave failed: {}", e), event_id: None,
                };
            }
        };

        let event_id = event.id.clone();
        self.network_service.add_event_to_dag(event).await;

        info!(address = %request.address, subnet_id = %request.subnet_id, event_id = %event_id, "Left subnet");
        LeaveSubnetResponse { success: true, message: "Left subnet".to_string(), event_id: Some(event_id) }
    }

    async fn check_membership(&self, address: &str, subnet_id: &str) -> CheckMembershipResponse {
        let membership_key = format!("user:{}:subnet:{}", address, subnet_id);
        let membership_oid = ObjectId::new(
            setu_hash_with_domain(b"SETU_MEMBERSHIP:", membership_key.as_bytes()),
        );

        match self.network_service.state_provider().get_object(&membership_oid) {
            Some(data) => {
                let v: serde_json::Value = serde_json::from_slice(&data).unwrap_or_default();
                CheckMembershipResponse {
                    is_member: true,
                    address: address.to_string(),
                    subnet_id: subnet_id.to_string(),
                    joined_at: v["joined_at"].as_u64(),
                }
            }
            None => CheckMembershipResponse {
                is_member: false,
                address: address.to_string(),
                subnet_id: subnet_id.to_string(),
                joined_at: None,
            },
        }
    }

    async fn get_user_subnets(&self, address: &str) -> GetUserSubnetsResponse {
        // Point-query across all registered subnets (O(subnet_count))
        let all_subnets = self.network_service.get_all_subnets();
        let mut joined = Vec::new();

        for subnet_info in &all_subnets {
            let membership_key = format!("user:{}:subnet:{}", address, subnet_info.subnet_id);
            let membership_oid = ObjectId::new(
                setu_hash_with_domain(b"SETU_MEMBERSHIP:", membership_key.as_bytes()),
            );
            if self.network_service.state_provider().get_object(&membership_oid).is_some() {
                joined.push(subnet_info.subnet_id.clone());
            }
        }

        GetUserSubnetsResponse {
            address: address.to_string(),
            subnets: joined,
        }
    }
}

