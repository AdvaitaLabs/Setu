//! Profile & Credential - Identity Display and Verification
//!
//! Design Philosophy:
//! - Profile is OPTIONAL identity display information (like ENS profile)
//! - Credential is an independent verifiable attestation
//! - Both are owned directly by Address, no SBT intermediate layer
//! - Address is the root of ownership

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::object::{generate_object_id, Address, Object};

// ============================================================================
// Profile - Optional Identity Display
// ============================================================================

/// Profile data - Optional identity display information
///
/// This is similar to ENS profiles or social media profiles.
/// An Address can have zero or one Profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileData {
    /// Owner address
    pub owner: Address,

    /// Display name
    pub display_name: Option<String>,

    /// Avatar URL (IPFS hash or HTTP URL)
    pub avatar_url: Option<String>,

    /// Bio/Description
    pub bio: Option<String>,

    /// Custom attributes (website, twitter, etc.)
    pub attributes: HashMap<String, String>,

    /// Creation timestamp (milliseconds)
    pub created_at: u64,

    /// Last update timestamp (milliseconds)
    pub updated_at: u64,
}

/// Profile type alias
pub type Profile = Object<ProfileData>;

impl ProfileData {
    /// Create new profile data
    pub fn new(owner: Address) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            owner,
            display_name: None,
            avatar_url: None,
            bio: None,
            attributes: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn set_display_name(&mut self, name: impl Into<String>) {
        self.display_name = Some(name.into());
        self.touch();
    }

    pub fn set_avatar(&mut self, url: impl Into<String>) {
        self.avatar_url = Some(url.into());
        self.touch();
    }

    pub fn set_bio(&mut self, bio: impl Into<String>) {
        self.bio = Some(bio.into());
        self.touch();
    }

    pub fn set_attribute(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.attributes.insert(key.into(), value.into());
        self.touch();
    }

    pub fn get_attribute(&self, key: &str) -> Option<&String> {
        self.attributes.get(key)
    }

    pub fn remove_attribute(&mut self, key: &str) -> Option<String> {
        let result = self.attributes.remove(key);
        if result.is_some() {
            self.touch();
        }
        result
    }

    fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }
}

impl Profile {
    /// Create a new Profile object
    pub fn new(owner: Address) -> Self {
        let id = generate_object_id(format!("profile:{}", owner).as_bytes());
        let data = ProfileData::new(owner.clone());
        Object::new_owned(id, owner, data)
    }
}

/// Create a new profile for an address
pub fn create_profile(owner: Address) -> Profile {
    Profile::new(owner)
}

// ============================================================================
// Credential - Verifiable Attestation
// ============================================================================

/// Credential status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialStatus {
    /// Active and valid
    Active,
    /// Revoked by issuer
    Revoked,
    /// Expired (based on expires_at)
    Expired,
}

/// Credential data - Verifiable attestation
///
/// Credentials are issued by trusted parties (issuers) and can be:
/// - KYC verification
/// - Membership proof
/// - Achievement badges
/// - Professional certifications
///
/// Each credential is an independent object that can be queried and verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialData {
    /// Credential holder (the Address that owns this credential)
    pub holder: Address,

    /// Credential type (e.g., "kyc", "membership", "achievement")
    pub credential_type: String,

    /// Issuer address (who issued this credential)
    pub issuer: Address,

    /// Issue timestamp (milliseconds)
    pub issued_at: u64,

    /// Expiration timestamp (None = never expires)
    pub expires_at: Option<u64>,

    /// Credential status
    pub status: CredentialStatus,

    /// Credential data/claims (key-value pairs)
    pub claims: HashMap<String, String>,

    /// Optional metadata (e.g., proof, signature reference)
    pub metadata: HashMap<String, String>,
}

/// Credential type alias
pub type Credential = Object<CredentialData>;

impl CredentialData {
    /// Create new credential data
    pub fn new(holder: Address, credential_type: impl Into<String>, issuer: Address) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            holder,
            credential_type: credential_type.into(),
            issuer,
            issued_at: now,
            expires_at: None,
            status: CredentialStatus::Active,
            claims: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Check if credential is valid (active and not expired)
    pub fn is_valid(&self) -> bool {
        if self.status != CredentialStatus::Active {
            return false;
        }

        if let Some(expires_at) = self.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            if now > expires_at {
                return false;
            }
        }

        true
    }

    /// Set expiration time
    pub fn set_expiry(&mut self, expires_at: u64) {
        self.expires_at = Some(expires_at);
    }

    /// Revoke credential (only issuer should call this)
    pub fn revoke(&mut self) {
        self.status = CredentialStatus::Revoked;
    }

    /// Add a claim
    pub fn add_claim(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.claims.insert(key.into(), value.into());
    }

    /// Get a claim
    pub fn get_claim(&self, key: &str) -> Option<&String> {
        self.claims.get(key)
    }
}

impl Credential {
    /// Create a new Credential object
    pub fn new(holder: Address, credential_type: impl Into<String>, issuer: Address) -> Self {
        let cred_type = credential_type.into();
        let id = generate_object_id(
            format!("credential:{}:{}:{}", holder, issuer, cred_type).as_bytes(),
        );
        let data = CredentialData::new(holder.clone(), cred_type, issuer);
        // Credentials are owned by the holder but issued by issuer
        Object::new_owned(id, holder, data)
    }

    /// Create with expiration
    pub fn new_with_expiry(
        holder: Address,
        credential_type: impl Into<String>,
        issuer: Address,
        expires_at: u64,
    ) -> Self {
        let mut cred = Self::new(holder, credential_type, issuer);
        cred.data.set_expiry(expires_at);
        cred
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a KYC credential
pub fn create_kyc_credential(holder: Address, issuer: Address, level: &str) -> Credential {
    let mut cred = Credential::new(holder, "kyc", issuer);
    cred.data.add_claim("level", level);
    cred
}

/// Create a membership credential
pub fn create_membership_credential(
    holder: Address,
    issuer: Address,
    organization: &str,
    expires_at: Option<u64>,
) -> Credential {
    let mut cred = Credential::new(holder, "membership", issuer);
    cred.data.add_claim("organization", organization);
    if let Some(exp) = expires_at {
        cred.data.set_expiry(exp);
    }
    cred
}

/// Create an achievement credential
pub fn create_achievement_credential(
    holder: Address,
    issuer: Address,
    achievement: &str,
    description: &str,
) -> Credential {
    let mut cred = Credential::new(holder, "achievement", issuer);
    cred.data.add_claim("name", achievement);
    cred.data.add_claim("description", description);
    cred
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_profile() {
        let owner = Address::from_str_id("alice");
        let mut profile = create_profile(owner.clone());

        assert_eq!(profile.data.owner, owner);
        assert!(profile.data.display_name.is_none());

        profile.data.set_display_name("Alice");
        profile.data.set_bio("Hello, World!");
        profile.data.set_attribute("twitter", "@alice");

        assert_eq!(profile.data.display_name, Some("Alice".to_string()));
        assert_eq!(profile.data.bio, Some("Hello, World!".to_string()));
        assert_eq!(
            profile.data.get_attribute("twitter"),
            Some(&"@alice".to_string())
        );
    }

    #[test]
    fn test_create_credential() {
        let holder = Address::from_str_id("alice");
        let issuer = Address::from_str_id("kyc_provider");

        let cred = create_kyc_credential(holder.clone(), issuer.clone(), "level_2");

        assert_eq!(cred.data.holder, holder);
        assert_eq!(cred.data.issuer, issuer);
        assert_eq!(cred.data.credential_type, "kyc");
        assert_eq!(cred.data.get_claim("level"), Some(&"level_2".to_string()));
        assert!(cred.data.is_valid());
    }

    #[test]
    fn test_credential_expiry() {
        let holder = Address::from_str_id("bob");
        let issuer = Address::from_str_id("org");

        // Create credential that expires in the past
        let mut cred = Credential::new(holder, "test", issuer);
        cred.data.set_expiry(1000); // Very old timestamp

        assert!(!cred.data.is_valid()); // Should be expired
    }

    #[test]
    fn test_credential_revoke() {
        let holder = Address::from_str_id("charlie");
        let issuer = Address::from_str_id("authority");

        let mut cred = Credential::new(holder, "certificate", issuer);
        assert!(cred.data.is_valid());

        cred.data.revoke();
        assert!(!cred.data.is_valid());
        assert_eq!(cred.data.status, CredentialStatus::Revoked);
    }

    #[test]
    fn test_membership_credential() {
        let holder = Address::from_str_id("member");
        let issuer = Address::from_str_id("dao");

        let cred = create_membership_credential(holder.clone(), issuer.clone(), "CryptoDAO", None);

        assert_eq!(cred.data.credential_type, "membership");
        assert_eq!(
            cred.data.get_claim("organization"),
            Some(&"CryptoDAO".to_string())
        );
        assert!(cred.data.expires_at.is_none());
    }
}
