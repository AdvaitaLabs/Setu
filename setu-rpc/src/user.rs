//! User RPC Service - API for wallet and client applications
//!
//! This module provides RPC interfaces for:
//! - User registration
//! - Account queries (Flux, Power, Dread balances)
//! - Relation network queries
//! - Subnet membership queries
//!
//! These APIs are designed to be consumed by:
//! - Wallet plugins (like MetaMask-style browser extensions)
//! - Mobile wallets
//! - DApps
//! - CLI tools

use serde::{Deserialize, Serialize};

// ============================================================================
// User Registration
// ============================================================================

/// Request to register a new user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterUserRequest {
    /// User's unique identifier (derived from public key)
    pub user_id: String,
    /// User's public key for authentication
    pub public_key: Vec<u8>,
    /// Subnet ID to register in (None = root subnet)
    pub subnet_id: Option<String>,
    /// Optional display name
    pub display_name: Option<String>,
    /// Optional metadata (JSON string)
    pub metadata: Option<String>,
    /// Initial power allocation (if allowed)
    pub initial_power: Option<u64>,
    /// Inviter's address (who invited this user)
    pub invited_by: Option<String>,
    /// Invite code used for registration
    pub invite_code: Option<String>,
}

/// Response to user registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterUserResponse {
    /// Whether registration was successful
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Assigned user address
    pub user_address: Option<String>,
    /// Event ID for this registration
    pub event_id: Option<String>,
}

// ============================================================================
// Account Queries
// ============================================================================

/// Request to get user account information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAccountRequest {
    /// User's address
    pub address: String,
}

/// User profile information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileInfo {
    /// Display name
    pub display_name: Option<String>,
    /// Avatar URL
    pub avatar_url: Option<String>,
    /// Bio/description
    pub bio: Option<String>,
    /// Creation timestamp
    pub created_at: u64,
}

/// Response with user account information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAccountResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Flux balance (main transferable token)
    pub flux_balance: u64,
    /// Power value (computational/voting power)
    pub power: u64,
    /// Dread value (reputation/penalty score)
    pub dread: u64,
    /// User profile information
    pub profile: Option<ProfileInfo>,
    /// Number of relations in the user's network
    pub relation_count: u64,
    /// Number of subnets the user has joined
    pub subnet_count: u64,
    /// Number of credentials the user holds
    pub credential_count: u64,
}

/// Request to get user balance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceRequest {
    /// User's address
    pub address: String,
    /// Optional coin type filter (None = all types)
    pub coin_type: Option<String>,
}

/// Balance information for a coin type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinBalance {
    /// Coin type (e.g., "FLUX", "SETU")
    pub coin_type: String,
    /// Total balance
    pub balance: u64,
    /// Number of coin objects
    pub coin_count: u32,
}

/// Response with user balance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Balances by coin type
    pub balances: Vec<CoinBalance>,
    /// Total balance across all coin types
    pub total_balance: u64,
}

/// Request to get user power
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPowerRequest {
    /// User's address
    pub address: String,
}

/// Response with user power information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPowerResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Current power value
    pub power: u64,
    /// Power rank (if available)
    pub rank: Option<u64>,
    /// Power history (recent changes)
    pub recent_changes: Vec<PowerChange>,
}

/// A power change record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerChange {
    /// Amount changed (positive or negative)
    pub amount: i64,
    /// Reason for change
    pub reason: String,
    /// Timestamp
    pub timestamp: u64,
    /// Related event ID
    pub event_id: Option<String>,
}

/// Request to get user dread
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetDreadRequest {
    /// User's address
    pub address: String,
}

/// Response with user dread information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetDreadResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Current dread value
    pub dread: u64,
    /// Dread level/tier
    pub level: Option<String>,
    /// Recent dread changes
    pub recent_changes: Vec<DreadChange>,
}

/// A dread change record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreadChange {
    /// Amount changed
    pub amount: i64,
    /// Reason for change
    pub reason: String,
    /// Timestamp
    pub timestamp: u64,
    /// Related event ID
    pub event_id: Option<String>,
}

// ============================================================================
// Relation Network Queries
// ============================================================================

/// Request to get user relations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRelationsRequest {
    /// User's address
    pub address: String,
    /// Optional filter by relation type
    pub relation_type: Option<String>,
    /// Maximum number of relations to return
    pub limit: Option<u32>,
    /// Offset for pagination
    pub offset: Option<u32>,
}

/// A relation in the user's network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationInfo {
    /// Target user's address
    pub target_address: String,
    /// Target user's display name (if available)
    pub target_display_name: Option<String>,
    /// Relation type
    pub relation_type: String,
    /// Relation weight/strength
    pub weight: u32,
    /// When the relation was established
    pub established_at: u64,
}

/// Response with user relations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRelationsResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Relations list
    pub relations: Vec<RelationInfo>,
    /// Total relation count (for pagination)
    pub total_count: u64,
    /// Inviter information (if any)
    pub invited_by: Option<String>,
    /// Number of users this user has invited
    pub invite_count: u64,
}

// ============================================================================
// Credential Queries
// ============================================================================

/// Request to get user credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCredentialsRequest {
    /// User's address
    pub address: String,
    /// Optional filter by credential type
    pub credential_type: Option<String>,
    /// Whether to include expired credentials
    pub include_expired: bool,
}

/// Credential information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialInfo {
    /// Credential ID
    pub credential_id: String,
    /// Credential type (e.g., "kyc", "membership")
    pub credential_type: String,
    /// Issuer address
    pub issuer: String,
    /// Issue timestamp
    pub issued_at: u64,
    /// Expiration timestamp (if any)
    pub expires_at: Option<u64>,
    /// Whether the credential is currently valid
    pub is_valid: bool,
    /// Credential claims (key-value pairs)
    pub claims: std::collections::HashMap<String, String>,
}

/// Response with user credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCredentialsResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Credentials list
    pub credentials: Vec<CredentialInfo>,
    /// Number of valid credentials
    pub valid_count: u64,
}

// ============================================================================
// Subnet Queries
// ============================================================================

/// Request to get user's subnet memberships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSubnetsRequest {
    /// User's address
    pub address: String,
}

/// Subnet membership information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubnetMembershipInfo {
    /// Subnet ID
    pub subnet_id: String,
    /// Subnet name
    pub subnet_name: Option<String>,
    /// When the user joined
    pub joined_at: u64,
    /// Last activity timestamp
    pub last_activity: u64,
    /// Whether this is the user's primary subnet
    pub is_primary: bool,
    /// Number of interactions in this subnet
    pub interaction_count: u64,
    /// Number of local relations in this subnet
    pub local_relation_count: u64,
}

/// Response with user's subnet memberships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSubnetsResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Subnet memberships
    pub subnets: Vec<SubnetMembershipInfo>,
    /// Primary subnet ID
    pub primary_subnet: Option<String>,
}

// ============================================================================
// Transfer Operations
// ============================================================================

/// Request to transfer Flux to another user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    /// Sender's address
    pub from: String,
    /// Recipient's address
    pub to: String,
    /// Amount to transfer
    pub amount: u64,
    /// Coin type (default: "FLUX")
    pub coin_type: Option<String>,
    /// Optional memo/note
    pub memo: Option<String>,
    /// Signature for authentication
    pub signature: Vec<u8>,
}

/// Response to transfer request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResponse {
    /// Whether the transfer was submitted successfully
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Event ID for this transfer
    pub event_id: Option<String>,
    /// Estimated confirmation time (seconds)
    pub estimated_confirmation: Option<u64>,
}

// ============================================================================
// Relation Operations
// ============================================================================

/// Request to add a relation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddRelationRequest {
    /// User's address (who is adding the relation)
    pub from: String,
    /// Target user's address
    pub to: String,
    /// Relation type
    pub relation_type: String,
    /// Optional weight (default: 50)
    pub weight: Option<u32>,
    /// Signature for authentication
    pub signature: Vec<u8>,
}

/// Response to add relation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddRelationResponse {
    /// Whether the relation was added successfully
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Event ID for this operation
    pub event_id: Option<String>,
}

// ============================================================================
// User RPC Handler Trait
// ============================================================================

/// Trait for handling user RPC requests
/// 
/// Implement this trait to provide user-related RPC services.
/// This is designed to be used by validators or dedicated API servers.
#[async_trait::async_trait]
pub trait UserRpcHandler: Send + Sync {
    // ========== Registration ==========
    
    /// Register a new user
    async fn register_user(&self, request: RegisterUserRequest) -> RegisterUserResponse;
    
    // ========== Account Queries ==========
    
    /// Get user account information
    async fn get_account(&self, request: GetAccountRequest) -> GetAccountResponse;
    
    /// Get user balance
    async fn get_balance(&self, request: GetBalanceRequest) -> GetBalanceResponse;
    
    /// Get user power
    async fn get_power(&self, request: GetPowerRequest) -> GetPowerResponse;
    
    /// Get user dread
    async fn get_dread(&self, request: GetDreadRequest) -> GetDreadResponse;
    
    // ========== Relation Queries ==========
    
    /// Get user relations
    async fn get_relations(&self, request: GetRelationsRequest) -> GetRelationsResponse;
    
    // ========== Credential Queries ==========
    
    /// Get user credentials
    async fn get_credentials(&self, request: GetCredentialsRequest) -> GetCredentialsResponse;
    
    // ========== Subnet Queries ==========
    
    /// Get user's subnet memberships
    async fn get_subnets(&self, request: GetSubnetsRequest) -> GetSubnetsResponse;
    
    // ========== Operations ==========
    
    /// Transfer Flux to another user
    async fn transfer(&self, request: TransferRequest) -> TransferResponse;
    
    /// Add a relation to another user
    async fn add_relation(&self, request: AddRelationRequest) -> AddRelationResponse;
}

// ============================================================================
// User RPC Request/Response Wrappers
// ============================================================================

/// Wrapper enum for all user RPC request types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserRpcRequest {
    RegisterUser(RegisterUserRequest),
    GetAccount(GetAccountRequest),
    GetBalance(GetBalanceRequest),
    GetPower(GetPowerRequest),
    GetDread(GetDreadRequest),
    GetRelations(GetRelationsRequest),
    GetCredentials(GetCredentialsRequest),
    GetSubnets(GetSubnetsRequest),
    Transfer(TransferRequest),
    AddRelation(AddRelationRequest),
}

/// Wrapper enum for all user RPC response types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserRpcResponse {
    RegisterUser(RegisterUserResponse),
    GetAccount(GetAccountResponse),
    GetBalance(GetBalanceResponse),
    GetPower(GetPowerResponse),
    GetDread(GetDreadResponse),
    GetRelations(GetRelationsResponse),
    GetCredentials(GetCredentialsResponse),
    GetSubnets(GetSubnetsResponse),
    Transfer(TransferResponse),
    AddRelation(AddRelationResponse),
    Error(String),
}

impl UserRpcRequest {
    /// Serialize request to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }
    
    /// Deserialize request from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

impl UserRpcResponse {
    /// Serialize response to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }
    
    /// Deserialize response from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_register_user_request_serialization() {
        let request = RegisterUserRequest {
            user_id: "user-123".to_string(),
            public_key: vec![1, 2, 3, 4],
            subnet_id: Some("subnet-1".to_string()),
            display_name: Some("Alice".to_string()),
            metadata: None,
            initial_power: Some(100),
            invited_by: Some("user-456".to_string()),
            invite_code: Some("INVITE123".to_string()),
        };
        
        let wrapped = UserRpcRequest::RegisterUser(request);
        let bytes = wrapped.to_bytes().unwrap();
        let decoded = UserRpcRequest::from_bytes(&bytes).unwrap();
        
        match decoded {
            UserRpcRequest::RegisterUser(req) => {
                assert_eq!(req.user_id, "user-123");
                assert_eq!(req.display_name, Some("Alice".to_string()));
                assert_eq!(req.invited_by, Some("user-456".to_string()));
            }
            _ => panic!("Wrong request type"),
        }
    }
    
    #[test]
    fn test_get_account_response_serialization() {
        let response = GetAccountResponse {
            found: true,
            address: "0x123".to_string(),
            flux_balance: 1000,
            power: 50,
            dread: 0,
            profile: Some(ProfileInfo {
                display_name: Some("Alice".to_string()),
                avatar_url: None,
                bio: Some("Hello!".to_string()),
                created_at: 1234567890,
            }),
            relation_count: 10,
            subnet_count: 2,
            credential_count: 1,
        };
        
        let wrapped = UserRpcResponse::GetAccount(response);
        let bytes = wrapped.to_bytes().unwrap();
        let decoded = UserRpcResponse::from_bytes(&bytes).unwrap();
        
        match decoded {
            UserRpcResponse::GetAccount(resp) => {
                assert!(resp.found);
                assert_eq!(resp.flux_balance, 1000);
                assert_eq!(resp.power, 50);
            }
            _ => panic!("Wrong response type"),
        }
    }
}

