use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

/// 32-byte object identifier
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ObjectId([u8; 32]);

impl ObjectId {
    pub const ZERO: ObjectId = ObjectId([0u8; 32]);

    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != 32 {
            return Err("ObjectId must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, &'static str> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes = hex::decode(hex_str).map_err(|_| "Invalid hex string")?;
        Self::from_bytes(&bytes)
    }

    pub fn random() -> Self {
        use sha2::Digest;
        let mut hasher = Sha256::new();
        hasher.update(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                .to_le_bytes(),
        );
        // Add some entropy from memory address
        let entropy: usize = &hasher as *const _ as usize;
        hasher.update(entropy.to_le_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self)
    }
}

impl FromStr for ObjectId {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl AsRef<[u8]> for ObjectId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

// bincode::Encode implementation for ObjectId
impl bincode::Encode for ObjectId {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        bincode::Encode::encode(&self.0, encoder)
    }
}

impl<C> bincode::Decode<C> for ObjectId {
    fn decode<D: bincode::de::Decoder<Context = C>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self(<[u8; 32]>::decode(decoder)?))
    }
}

impl<'de, C> bincode::BorrowDecode<'de, C> for ObjectId {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = C>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self(<[u8; 32]>::borrow_decode(decoder)?))
    }
}

/// 32-byte address
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Address([u8; 32]);

impl Address {
    pub const ZERO: Address = Address([0u8; 32]);

    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != 32 {
            return Err("Address must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, &'static str> {
        let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let bytes = hex::decode(hex_str).map_err(|_| "Invalid hex string")?;
        Self::from_bytes(&bytes)
    }

    /// Create address from a string identifier (hashes the string)
    pub fn from_str_id(id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(id.as_bytes());
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self)
    }
}

impl FromStr for Address {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl From<&str> for Address {
    fn from(s: &str) -> Self {
        Self::from_str_id(s)
    }
}

impl AsRef<[u8]> for Address {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

// bincode::Encode implementation for Address
impl bincode::Encode for Address {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        bincode::Encode::encode(&self.0, encoder)
    }
}

impl<C> bincode::Decode<C> for Address {
    fn decode<D: bincode::de::Decoder<Context = C>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self(<[u8; 32]>::decode(decoder)?))
    }
}

impl<'de, C> bincode::BorrowDecode<'de, C> for Address {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = C>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self(<[u8; 32]>::borrow_decode(decoder)?))
    }
}

/// 32-byte object digest (content hash)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ObjectDigest([u8; 32]);

impl ObjectDigest {
    pub const ZERO: ObjectDigest = ObjectDigest([0u8; 32]);

    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != 32 {
            return Err("ObjectDigest must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ObjectDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl fmt::Debug for ObjectDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectDigest({})", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObjectType {
    OwnedObject,
    SharedObject,
    ImmutableObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ownership {
    AddressOwner(Address),
    ObjectOwner(ObjectId),
    Shared { initial_shared_version: u64 },
    Immutable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMetadata {
    pub id: ObjectId,
    pub version: u64,
    pub digest: ObjectDigest,
    pub object_type: ObjectType,
    pub owner: Option<Address>,
    pub ownership: Ownership,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object<T: Serialize + Clone> {
    pub metadata: ObjectMetadata,
    pub data: T,
}

impl<T: Serialize + Clone> Object<T> {
    pub fn new_owned(id: ObjectId, owner: Address, data: T) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut obj = Self {
            metadata: ObjectMetadata {
                id,
                version: 1,
                digest: ObjectDigest::ZERO,
                object_type: ObjectType::OwnedObject,
                owner: Some(owner),
                ownership: Ownership::AddressOwner(owner),
                created_at: now,
                updated_at: now,
            },
            data,
        };
        obj.compute_digest();
        obj
    }

    pub fn new_shared(id: ObjectId, data: T, initial_version: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut obj = Self {
            metadata: ObjectMetadata {
                id,
                version: initial_version,
                digest: ObjectDigest::ZERO,
                object_type: ObjectType::SharedObject,
                owner: None,
                ownership: Ownership::Shared {
                    initial_shared_version: initial_version,
                },
                created_at: now,
                updated_at: now,
            },
            data,
        };
        obj.compute_digest();
        obj
    }

    pub fn new_immutable(id: ObjectId, data: T) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut obj = Self {
            metadata: ObjectMetadata {
                id,
                version: 1,
                digest: ObjectDigest::ZERO,
                object_type: ObjectType::ImmutableObject,
                owner: None,
                ownership: Ownership::Immutable,
                created_at: now,
                updated_at: now,
            },
            data,
        };
        obj.compute_digest();
        obj
    }

    pub fn id(&self) -> &ObjectId {
        &self.metadata.id
    }

    pub fn version(&self) -> u64 {
        self.metadata.version
    }

    pub fn digest(&self) -> &ObjectDigest {
        &self.metadata.digest
    }

    pub fn owner(&self) -> Option<&Address> {
        self.metadata.owner.as_ref()
    }

    pub fn is_owned(&self) -> bool {
        self.metadata.object_type == ObjectType::OwnedObject
    }

    pub fn is_shared(&self) -> bool {
        self.metadata.object_type == ObjectType::SharedObject
    }

    pub fn is_immutable(&self) -> bool {
        self.metadata.object_type == ObjectType::ImmutableObject
    }

    pub fn is_owned_by(&self, address: &Address) -> bool {
        self.metadata.owner.as_ref() == Some(address)
    }

    /// Compute and update the object digest
    pub fn compute_digest(&mut self) {
        let mut hasher = Sha256::new();
        hasher.update(self.metadata.id.as_bytes());
        hasher.update(self.metadata.version.to_le_bytes());
        if let Ok(data_bytes) = bcs::to_bytes(&self.data) {
            hasher.update(&data_bytes);
        }
        let result = hasher.finalize();
        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(&result);
        self.metadata.digest = ObjectDigest::new(digest_bytes);
    }

    pub fn increment_version(&mut self) {
        self.metadata.version += 1;
        self.metadata.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.compute_digest();
    }

    pub fn transfer_to(&mut self, new_owner: Address) {
        if self.is_owned() {
            self.metadata.owner = Some(new_owner);
            self.metadata.ownership = Ownership::AddressOwner(new_owner);
            self.increment_version();
        }
    }
}

pub fn generate_object_id(seed: &[u8]) -> ObjectId {
    let mut hasher = Sha256::new();
    hasher.update(seed);
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_le_bytes(),
    );
    let result = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&result);
    ObjectId::new(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestData {
        value: u64,
    }

    #[test]
    fn test_owned_object() {
        let obj = Object::new_owned(
            generate_object_id(b"obj1"),
            Address::from_str_id("alice"),
            TestData { value: 100 },
        );
        assert!(obj.is_owned());
        assert_eq!(obj.version(), 1);
        assert_ne!(obj.digest(), &ObjectDigest::ZERO);
    }

    #[test]
    fn test_shared_object() {
        let obj = Object::new_shared(generate_object_id(b"obj2"), TestData { value: 200 }, 1);
        assert!(obj.is_shared());
    }

    #[test]
    fn test_transfer() {
        let mut obj = Object::new_owned(
            generate_object_id(b"obj3"),
            Address::from_str_id("alice"),
            TestData { value: 100 },
        );
        obj.transfer_to(Address::from_str_id("bob"));
        assert_eq!(obj.version(), 2);
    }

    #[test]
    fn test_object_id() {
        let id = ObjectId::random();
        let hex_str = id.to_string();
        let parsed = ObjectId::from_hex(&hex_str).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_address() {
        let addr = Address::from_str_id("alice");
        assert_ne!(addr, Address::ZERO);

        let addr2 = Address::from_str_id("alice");
        assert_eq!(addr, addr2);
    }
}
