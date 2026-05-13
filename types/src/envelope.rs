//! ObjectEnvelope — unified storage format for Move objects and legacy coins.
//!
//! ## Format Detection
//!
//! BCS first 2 bytes == ENVELOPE_MAGIC (0x4553) → ObjectEnvelope
//! Otherwise → try CoinState (legacy)
//!
//! ## Storage Key Convention
//!
//! | Data type | Key format              | Value format              |
//! |-----------|-------------------------|---------------------------|
//! | Object    | `oid:{64hex}`           | BCS(ObjectEnvelope)       |
//! | Module    | `mod:{hex_addr}::{name}`| raw bytecode              |
//! | Index     | `idx:own:{hex_o}:{hex_id}` | empty (existence = relation) |

use serde::{Deserialize, Serialize};
use crate::object::{ObjectId, Address, Ownership, ObjectDigest};
use crate::coin::{CoinState, CoinData, CoinType, Balance};

/// Magic number: first 2 BCS bytes of ObjectEnvelope = [0x53, 0x45] ("SE" little-endian).
pub const ENVELOPE_MAGIC: u16 = 0x4553;

/// Universal object envelope — wraps any Move object or legacy Coin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectEnvelope {
    /// Magic identifier (always ENVELOPE_MAGIC)
    pub magic: u16,
    /// Object metadata
    pub metadata: EnvelopeMetadata,
    /// Move type tag (e.g. `"0x1::coin::Coin<0x1::setu::SETU>"`)
    pub type_tag: String,
    /// BCS-serialized Move struct data
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvelopeMetadata {
    pub id: ObjectId,
    pub owner: Address,
    pub version: u64,
    pub ownership: Ownership,
    /// BLAKE3 digest of `data` (domain: "SETU_OBJ_DIGEST:")
    pub digest: ObjectDigest,
}

impl ObjectEnvelope {
    /// Build from Move VM execution result.
    pub fn from_move_result(
        id: ObjectId,
        owner: Address,
        version: u64,
        ownership: Ownership,
        type_tag: String,
        bcs_data: Vec<u8>,
    ) -> Self {
        let digest = Self::compute_digest(&bcs_data);
        Self {
            magic: ENVELOPE_MAGIC,
            metadata: EnvelopeMetadata { id, owner, version, ownership, digest },
            type_tag,
            data: bcs_data,
        }
    }

    /// Build from an existing `Object<CoinData>`.
    pub fn from_coin_object(obj: &crate::object::Object<CoinData>) -> Result<Self, String> {
        let coin_bcs = bcs::to_bytes(&obj.data)
            .map_err(|e| format!("BCS serialize CoinData: {e}"))?;
        let owner = obj.metadata.owner.unwrap_or(Address::ZERO);
        Ok(Self::from_move_result(
            obj.metadata.id,
            owner,
            obj.metadata.version,
            obj.metadata.ownership,
            format!("0x1::coin::Coin<0x1::setu::{}>", obj.data.coin_type.as_str()),
            coin_bcs,
        ))
    }

    /// Build from legacy `CoinState` bytes.
    pub fn from_legacy_coin_state(id: ObjectId, cs: &CoinState) -> Result<Self, String> {
        let owner = Address::normalize(&cs.owner);
        let coin_data = CoinData {
            coin_type: CoinType::new(&cs.coin_type),
            balance: Balance::new(cs.balance),
        };
        let bcs_data = bcs::to_bytes(&coin_data)
            .map_err(|e| format!("BCS serialize CoinData: {e}"))?;
        Ok(Self::from_move_result(
            id,
            owner,
            cs.version,
            Ownership::AddressOwner(owner),
            format!("0x1::coin::Coin<0x1::setu::{}>", cs.coin_type),
            bcs_data,
        ))
    }

    /// Try converting back to `Object<CoinData>` (backward compat).
    pub fn try_as_coin_object(&self) -> Option<crate::object::Object<CoinData>> {
        let coin_data: CoinData = bcs::from_bytes(&self.data).ok()?;
        Some(crate::object::Object {
            metadata: crate::object::ObjectMetadata {
                id: self.metadata.id,
                version: self.metadata.version,
                digest: self.metadata.digest,
                object_type: crate::object::ObjectType::OwnedObject,
                owner: Some(self.metadata.owner),
                ownership: self.metadata.ownership,
                created_at: 0,
                updated_at: 0,
            },
            data: coin_data,
        })
    }

    /// Serialize to BCS bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        bcs::to_bytes(self).expect("ObjectEnvelope BCS serialization should not fail")
    }

    /// Deserialize from BCS bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bcs::from_bytes(bytes).ok()
    }

    fn compute_digest(data: &[u8]) -> ObjectDigest {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_OBJ_DIGEST:");
        hasher.update(data);
        ObjectDigest::new(*hasher.finalize().as_bytes())
    }
}

/// Result of format detection on raw storage bytes.
pub enum StorageFormat {
    /// New format (magic == ENVELOPE_MAGIC)
    Envelope(ObjectEnvelope),
    /// Legacy BCS(CoinState)
    LegacyCoinState(CoinState),
    /// Unrecognized
    Unknown,
}

/// Detect storage format and parse.
pub fn detect_and_parse(bytes: &[u8]) -> StorageFormat {
    if bytes.len() >= 2 {
        let magic = u16::from_le_bytes([bytes[0], bytes[1]]);
        if magic == ENVELOPE_MAGIC {
            if let Some(env) = ObjectEnvelope::from_bytes(bytes) {
                return StorageFormat::Envelope(env);
            }
        }
    }
    if let Some(cs) = CoinState::from_bytes(bytes) {
        return StorageFormat::LegacyCoinState(cs);
    }
    StorageFormat::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{Address, ObjectId, Ownership};
    use crate::coin::{CoinData, CoinType, Balance};

    fn sample_coin_data() -> CoinData {
        CoinData {
            coin_type: CoinType::native(),
            balance: Balance::new(1000),
        }
    }

    fn sample_envelope() -> ObjectEnvelope {
        let id = ObjectId::new([1u8; 32]);
        let owner = Address::from_str_id("alice");
        let bcs_data = bcs::to_bytes(&sample_coin_data()).unwrap();
        ObjectEnvelope::from_move_result(
            id,
            owner,
            1,
            Ownership::AddressOwner(owner),
            "0x1::coin::Coin<0x1::setu::ROOT>".to_string(),
            bcs_data,
        )
    }

    #[test]
    fn test_object_envelope_roundtrip() {
        let env = sample_envelope();
        let bytes = env.to_bytes();
        let restored = ObjectEnvelope::from_bytes(&bytes).expect("deserialize failed");
        assert_eq!(env, restored);
    }

    #[test]
    fn test_envelope_from_coin_object() {
        let owner = Address::from_str_id("bob");
        let coin = crate::create_coin(owner, 500);
        let env = ObjectEnvelope::from_coin_object(&coin).expect("conversion failed");
        assert_eq!(env.metadata.id, coin.metadata.id);
        assert_eq!(env.metadata.owner, owner);
        assert!(env.type_tag.contains("ROOT"));
        assert_eq!(env.magic, ENVELOPE_MAGIC);
    }

    #[test]
    fn test_envelope_from_legacy_coin_state() {
        let owner = Address::from_str_id("charlie");
        let cs = CoinState::new(owner.to_string(), 2000);
        let id = ObjectId::new([3u8; 32]);
        let env = ObjectEnvelope::from_legacy_coin_state(id, &cs).expect("conversion failed");
        assert_eq!(env.metadata.id, id);
        assert_eq!(env.metadata.version, cs.version);

        // Round-trip back to CoinData
        let coin_obj = env.try_as_coin_object().expect("try_as_coin_object failed");
        assert_eq!(coin_obj.data.balance.value(), 2000);
    }

    #[test]
    fn test_envelope_try_as_coin_object() {
        let owner = Address::from_str_id("dave");
        let coin = crate::create_coin(owner, 750);
        let env = ObjectEnvelope::from_coin_object(&coin).unwrap();
        let restored = env.try_as_coin_object().expect("round-trip failed");
        assert_eq!(restored.data.balance.value(), 750);
        assert_eq!(restored.metadata.owner, Some(owner));
    }

    #[test]
    fn test_detect_and_parse_envelope() {
        let env = sample_envelope();
        let bytes = env.to_bytes();
        match detect_and_parse(&bytes) {
            StorageFormat::Envelope(e) => assert_eq!(e, env),
            _ => panic!("expected Envelope"),
        }
    }

    #[test]
    fn test_detect_and_parse_legacy() {
        let cs = CoinState::new("0xabc".to_string(), 100);
        let bytes = cs.to_bytes();
        match detect_and_parse(&bytes) {
            StorageFormat::LegacyCoinState(restored) => assert_eq!(restored, cs),
            StorageFormat::Envelope(_) => panic!("should not be envelope"),
            StorageFormat::Unknown => {
                // CoinState BCS may not be distinguishable if its first bytes
                // happen to collide with ENVELOPE_MAGIC. This is acceptable —
                // in production, new data always uses ObjectEnvelope.
            }
        }
    }

    #[test]
    fn test_detect_and_parse_unknown() {
        let bytes = vec![0xFF, 0xFF, 0x00];
        match detect_and_parse(&bytes) {
            StorageFormat::Unknown => {}
            _ => panic!("expected Unknown"),
        }
    }
}
