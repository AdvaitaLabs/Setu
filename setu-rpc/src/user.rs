//! User RPC Service - API for wallet and client applications
//!
//! This module provides RPC interfaces for:
//! - User registration
//! - Account queries (Flux, Power, Credit balances)
//! - Credential queries
//! - Transfer operations
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
/// 
/// Supports two registration methods:
/// 1. MetaMask (Ethereum wallet): address + ECDSA signature
/// 2. Nostr: nostr_pubkey + Schnorr signature (address derived from pubkey)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterUserRequest {
    /// User's Ethereum-style address (0x...)
    /// - For MetaMask: directly from wallet
    /// - For Nostr: derived from nostr_pubkey
    pub address: String,
    
    /// Optional: Nostr public key (32 bytes, Schnorr x-only public key)
    /// Only present for Nostr-based registrations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nostr_pubkey: Option<Vec<u8>>,
    
    /// Optional: Signature proving ownership
    /// - For MetaMask: ECDSA signature (65 bytes)
    /// - For Nostr: Schnorr signature (64 bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Vec<u8>>,
    
    /// Optional: Signed message (for verification)
    /// Format: "Register to Setu: {timestamp}"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    
    /// Timestamp (for replay attack prevention)
    pub timestamp: u64,
    
    /// Subnet ID to register in (None = root subnet)
    pub subnet_id: Option<String>,
    
    /// Optional display name
    pub display_name: Option<String>,
    
    /// Optional metadata (JSON string)
    pub metadata: Option<String>,
    
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
    /// User's registered address (same as request)
    pub address: String,
    /// Event ID for this registration
    pub event_id: Option<String>,
    /// Initial Flux balance allocated
    pub initial_flux: u64,
    /// Initial Power allocated
    pub initial_power: u64,
    /// Initial Credit allocated
    pub initial_credit: u64,
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
    /// Credit value (reputation score)
    pub credit: u64,
    /// User profile information
    pub profile: Option<ProfileInfo>,
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

/// Request to get user credit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCreditRequest {
    /// User's address
    pub address: String,
}

/// Response with user credit information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCreditResponse {
    /// Whether the account was found
    pub found: bool,
    /// User's address
    pub address: String,
    /// Current credit value
    pub credit: u64,
    /// Credit level/tier
    pub level: Option<String>,
    /// Recent credit changes
    pub recent_changes: Vec<CreditChange>,
}

/// A credit change record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditChange {
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
    
    /// Get user credit
    async fn get_credit(&self, request: GetCreditRequest) -> GetCreditResponse;
    
    // ========== Credential Queries ==========
    
    /// Get user credentials
    async fn get_credentials(&self, request: GetCredentialsRequest) -> GetCredentialsResponse;
    
    // ========== Operations ==========
    
    /// Transfer Flux to another user
    async fn transfer(&self, request: TransferRequest) -> TransferResponse;
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
    GetCredit(GetCreditRequest),
    GetCredentials(GetCredentialsRequest),
    Transfer(TransferRequest),
}

/// Wrapper enum for all user RPC response types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserRpcResponse {
    RegisterUser(RegisterUserResponse),
    GetAccount(GetAccountResponse),
    GetBalance(GetBalanceResponse),
    GetPower(GetPowerResponse),
    GetCredit(GetCreditResponse),
    GetCredentials(GetCredentialsResponse),
    Transfer(TransferResponse),
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
        // Test MetaMask registration
        let metamask_request = RegisterUserRequest {
            address: "0x1234567890abcdef".to_string(),
            nostr_pubkey: None,
            signature: Some(vec![1; 65]), // ECDSA signature
            message: Some("Register to Setu: 1234567890".to_string()),
            timestamp: 1234567890,
            subnet_id: Some("subnet-1".to_string()),
            display_name: Some("Alice".to_string()),
            metadata: None,
            invite_code: Some("INVITE123".to_string()),
        };
        
        let wrapped = UserRpcRequest::RegisterUser(metamask_request);
        let bytes = wrapped.to_bytes().unwrap();
        let decoded = UserRpcRequest::from_bytes(&bytes).unwrap();
        
        match decoded {
            UserRpcRequest::RegisterUser(req) => {
                assert_eq!(req.address, "0x1234567890abcdef");
                assert!(req.nostr_pubkey.is_none());
                assert_eq!(req.signature.as_ref().unwrap().len(), 65);
                assert_eq!(req.timestamp, 1234567890);
                assert_eq!(req.display_name, Some("Alice".to_string()));
                assert_eq!(req.invite_code, Some("INVITE123".to_string()));
            }
            _ => panic!("Wrong request type"),
        }
        
        // Test Nostr registration
        let nostr_request = RegisterUserRequest {
            address: "0xabcdef1234567890".to_string(),
            nostr_pubkey: Some(vec![1; 32]),
            signature: Some(vec![5; 64]), // Schnorr signature
            message: None,
            timestamp: 1234567890,
            subnet_id: None,
            display_name: Some("Bob".to_string()),
            metadata: None,
            invite_code: None,
        };
        
        let wrapped = UserRpcRequest::RegisterUser(nostr_request);
        let bytes = wrapped.to_bytes().unwrap();
        let decoded = UserRpcRequest::from_bytes(&bytes).unwrap();
        
        match decoded {
            UserRpcRequest::RegisterUser(req) => {
                assert_eq!(req.address, "0xabcdef1234567890");
                assert_eq!(req.nostr_pubkey.as_ref().unwrap().len(), 32);
                assert_eq!(req.signature.as_ref().unwrap().len(), 64);
                assert_eq!(req.display_name, Some("Bob".to_string()));
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
            credit: 100,
            profile: Some(ProfileInfo {
                display_name: Some("Alice".to_string()),
                avatar_url: None,
                bio: Some("Hello!".to_string()),
                created_at: 1234567890,
            }),
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
                assert_eq!(resp.credit, 100);
            }
            _ => panic!("Wrong response type"),
        }
    }
}
