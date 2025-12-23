//! SBT Object - Core of Digital Identity
//! 
//! Design Philosophy:
//! - SBT represents a person! It's a digital identity
//! - SBT owns resources like Coins (assets) and RelationGraphs (social relationships)
//! - SBT contains identity attributes like credentials, reputation
//! - One Address can correspond to one primary SBT (or multiple identities)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::object::{Object, ObjectId, Address, generate_object_id};

/// Credential type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub credential_type: String,  // "kyc", "membership", "achievement", etc.
    pub issuer: Address,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
    pub data: HashMap<String, String>,
}

/// SBT data - Identity information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SBTData {
    /// Corresponding on-chain address (primary key, one Address corresponds to one primary SBT)
    pub owner: Address,
    
    /// Identity type
    pub identity_type: String,  // "personal", "organization", "dao", etc.
    
    /// Display name
    pub display_name: Option<String>,
    
    /// Avatar URL
    pub avatar_url: Option<String>,
    
    /// Bio
    pub bio: Option<String>,
    
    /// Custom attributes
    pub attributes: HashMap<String, String>,
    
    /// Credential list (KYC, achievements, memberships, etc.)
    pub credentials: Vec<Credential>,
    
    /// Reputation score
    pub reputation_score: u64,
    
    /// Creation time
    pub created_at: u64,
    
    /// Update time
    pub updated_at: u64,
    
    /// Whether transferable (usually SBT is non-transferable)
    pub transferable: bool,
}

/// SBT type alias
pub type SBT = Object<SBTData>;

impl SBTData {
    /// Create a new SBT identity
    pub fn new(owner: Address, identity_type: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            owner,
            identity_type,
            display_name: None,
            avatar_url: None,
            bio: None,
            attributes: HashMap::new(),
            credentials: Vec::new(),
            reputation_score: 0,
            created_at: now,
            updated_at: now,
            transferable: false,
        }
    }

    pub fn set_display_name(&mut self, name: String) {
        self.display_name = Some(name);
        self.touch();
    }

    pub fn set_avatar(&mut self, url: String) {
        self.avatar_url = Some(url);
        self.touch();
    }

    pub fn set_bio(&mut self, bio: String) {
        self.bio = Some(bio);
        self.touch();
    }

    pub fn set_attribute(&mut self, key: String, value: String) {
        self.attributes.insert(key, value);
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

    pub fn add_reputation(&mut self, points: u64) {
        self.reputation_score = self.reputation_score.saturating_add(points);
        self.touch();
    }

    pub fn sub_reputation(&mut self, points: u64) {
        self.reputation_score = self.reputation_score.saturating_sub(points);
        self.touch();
    }
    
    /// Add credential
    pub fn add_credential(&mut self, credential: Credential) {
        self.credentials.push(credential);
        self.touch();
    }
    
    /// Remove expired credentials
    pub fn remove_expired_credentials(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        self.credentials.retain(|c| {
            c.expires_at.map_or(true, |exp| exp > now)
        });
        self.touch();
    }
    
    /// Get credentials by type
    pub fn get_credentials_by_type(&self, credential_type: &str) -> Vec<&Credential> {
        self.credentials
            .iter()
            .filter(|c| c.credential_type == credential_type)
            .collect()
    }

    fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }
}

/// Helper function to create SBT object
pub fn create_sbt(owner: Address, identity_type: String) -> SBT {
    let id = generate_object_id(format!("sbt:{}:{}", owner, identity_type).as_bytes());
    let data = SBTData::new(owner.clone(), identity_type);
    Object::new_owned(id, &owner, data)
}

/// Create personal SBT
pub fn create_personal_sbt(owner: Address) -> SBT {
    create_sbt(owner, "personal".to_string())
}

/// Create organization SBT
pub fn create_organization_sbt(owner: Address) -> SBT {
    create_sbt(owner, "organization".to_string())
}

// Remove old SBTCredential definition, use new Credential
// Keep type alias for backward compatibility
#[deprecated(note = "Use Credential instead")]
pub type SBTCredential = Credential;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_sbt() {
        let sbt = create_personal_sbt("alice".to_string());
        assert_eq!(sbt.data.owner, "alice");
        assert_eq!(sbt.data.identity_type, "personal");
        assert!(!sbt.data.transferable);
        assert!(sbt.is_owned());
    }

    #[test]
    fn test_sbt_attributes() {
        let mut data = SBTData::new("alice".to_string(), "personal".to_string());
        data.set_display_name("Alice".to_string());
        data.set_attribute("twitter".to_string(), "@alice".to_string());

        assert_eq!(data.display_name, Some("Alice".to_string()));
        assert_eq!(data.get_attribute("twitter"), Some(&"@alice".to_string()));
    }

    #[test]
    fn test_reputation() {
        let mut data = SBTData::new("alice".to_string(), "personal".to_string());
        data.add_reputation(100);
        assert_eq!(data.reputation_score, 100);

        data.sub_reputation(30);
        assert_eq!(data.reputation_score, 70);
    }

    #[test]
    fn test_credential() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
            
        let cred = Credential {
            credential_type: "achievement".to_string(),
            issuer: "issuer".to_string(),
            issued_at: now,
            expires_at: None,
            data: [("name".to_string(), "First Transaction".to_string())]
                .iter()
                .cloned()
                .collect(),
        };

        assert_eq!(cred.credential_type, "achievement");
        assert_eq!(cred.issuer, "issuer");
    }
    
    #[test]
    fn test_add_credential() {
        let mut data = SBTData::new("alice".to_string(), "personal".to_string());
        
        let cred = Credential {
            credential_type: "kyc".to_string(),
            issuer: "kyc_provider".to_string(),
            issued_at: 1000,
            expires_at: Some(2000),
            data: HashMap::new(),
        };
        
        data.add_credential(cred);
        assert_eq!(data.credentials.len(), 1);
        assert_eq!(data.get_credentials_by_type("kyc").len(), 1);
    }
}
