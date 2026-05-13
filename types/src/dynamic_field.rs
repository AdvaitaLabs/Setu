//! Dynamic Fields — data types and deterministic OID derivation.
//!
//! See `docs/feat/dynamic-fields/design.md` v1.5.
//!
//! ## Storage layout
//! Each DF entry is a standalone SMT slot whose `ObjectEnvelope.data` is a
//! BCS-serialized [`DfFieldValue`], with envelope
//! `ownership = Ownership::ObjectOwner(parent_oid)`.
//!
//! ## OID derivation
//! ```text
//! df_oid = BLAKE3(b"SETU_DF:" || parent || canonical_type_tag || 0x00 || name_bcs)
//! ```
//! The canonical type tag string is used (rather than a Move-VM `TypeTag`) so
//! this crate stays free of the Move VM dependency (G4). VM-side callers
//! obtain the string via `TypeTag::to_canonical_string()`.

use serde::{Deserialize, Serialize};

use crate::object::ObjectId;

/// Move-side payload stored inside a DF entry's `ObjectEnvelope.data`.
///
/// `name_type_tag` and `value_type_tag` are canonical Move type tag strings
/// (e.g. `"u64"`, `"address"`, `"0xcafe::pool::Pair"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DfFieldValue {
    pub name_type_tag: String,
    pub name_bcs: Vec<u8>,
    pub value_type_tag: String,
    pub value_bcs: Vec<u8>,
}

/// Client-declared access intent for a dynamic field in a `MoveCall`.
///
/// - `Read`    → `borrow` / `exists_`
/// - `Mutate`  → `borrow_mut`
/// - `Create`  → `add` (entry must not exist)
/// - `Delete`  → `remove`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DfAccessMode {
    Read,
    Mutate,
    Create,
    Delete,
}

/// Domain separator for the BLAKE3 preimage. Never change this value without
/// a coordinated state migration — OIDs derived with a different prefix will
/// not collide with existing SMT entries but will silently break lookups.
const DF_OID_DOMAIN: &[u8] = b"SETU_DF:";

/// Derive the deterministic `ObjectId` of a dynamic field entry.
///
/// `name_type_tag` MUST be the canonical string form (see module docs).
/// `name_bcs` is the BCS-encoded key value.
///
/// The preimage layout is:
/// `b"SETU_DF:"` || `parent.as_bytes()` (32B)
///   || `name_type_tag.as_bytes()` || `0x00` || `name_bcs`
///
/// The `0x00` byte acts as a separator between the type tag string and the
/// key payload — canonical type tag strings never contain NUL, so this
/// removes the risk of `(tag="u6", bcs=b"4\x00...")` colliding with
/// `(tag="u64", bcs=b"...")`.
pub fn derive_df_oid(
    parent: &ObjectId,
    name_type_tag: &str,
    name_bcs: &[u8],
) -> ObjectId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DF_OID_DOMAIN);
    hasher.update(parent.as_bytes());
    hasher.update(name_type_tag.as_bytes());
    hasher.update(&[0u8]);
    hasher.update(name_bcs);
    ObjectId::new(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent(seed: u8) -> ObjectId {
        ObjectId::new([seed; 32])
    }

    #[test]
    fn derive_df_oid_deterministic() {
        let a = derive_df_oid(&parent(1), "u64", &[42, 0, 0, 0, 0, 0, 0, 0]);
        let b = derive_df_oid(&parent(1), "u64", &[42, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(a, b, "derive_df_oid must be deterministic");
    }

    #[test]
    fn derive_df_oid_different_parent() {
        let a = derive_df_oid(&parent(1), "u64", &[1, 0, 0, 0, 0, 0, 0, 0]);
        let b = derive_df_oid(&parent(2), "u64", &[1, 0, 0, 0, 0, 0, 0, 0]);
        assert_ne!(a, b, "different parent must derive different DF OID");
    }

    #[test]
    fn derive_df_oid_different_type_tag() {
        // Same 4-byte BCS payload: `42u32` and the first 4 bytes of `42u64`.
        // Without the type tag in the preimage these would collide.
        let bcs_u32_42 = [42u8, 0, 0, 0];
        let a = derive_df_oid(&parent(1), "u32", &bcs_u32_42);
        let b = derive_df_oid(&parent(1), "u64", &bcs_u32_42);
        assert_ne!(a, b, "same bcs under different type tag must not collide");
    }

    #[test]
    fn derive_df_oid_different_bcs() {
        let a = derive_df_oid(&parent(1), "u64", &[1, 0, 0, 0, 0, 0, 0, 0]);
        let b = derive_df_oid(&parent(1), "u64", &[2, 0, 0, 0, 0, 0, 0, 0]);
        assert_ne!(a, b, "different key bytes must derive different DF OID");
    }

    #[test]
    fn derive_df_oid_empty_bcs() {
        // Edge case: empty BCS payload (e.g. `unit`-like keys) must not
        // panic and must still produce a 32-byte ObjectId.
        let oid = derive_df_oid(&parent(1), "()", &[]);
        assert_eq!(oid.as_bytes().len(), 32);
    }

    #[test]
    fn derive_df_oid_known_preimage() {
        // Lock the hash preimage layout so regressions are caught.
        let p = parent(7);
        let tag = "u64";
        let bcs_bytes = [10u8, 0, 0, 0, 0, 0, 0, 0];

        let mut expected = blake3::Hasher::new();
        expected.update(b"SETU_DF:");
        expected.update(p.as_bytes());
        expected.update(tag.as_bytes());
        expected.update(&[0u8]);
        expected.update(&bcs_bytes);
        let expected_oid = ObjectId::new(*expected.finalize().as_bytes());

        let got = derive_df_oid(&p, tag, &bcs_bytes);
        assert_eq!(got, expected_oid);
    }

    #[test]
    fn df_field_value_bcs_roundtrip() {
        let v = DfFieldValue {
            name_type_tag: "u64".to_string(),
            name_bcs: vec![42, 0, 0, 0, 0, 0, 0, 0],
            value_type_tag: "0xcafe::pool::Liquidity".to_string(),
            value_bcs: vec![1, 2, 3, 4, 5],
        };
        let bytes = bcs::to_bytes(&v).expect("bcs encode");
        let decoded: DfFieldValue = bcs::from_bytes(&bytes).expect("bcs decode");
        assert_eq!(decoded, v);
    }

    #[test]
    fn df_access_mode_json_shape() {
        // Default untagged externally-tagged serde enum → bare variant name
        // string. Client SDKs rely on this exact wire shape.
        assert_eq!(serde_json::to_string(&DfAccessMode::Read).unwrap(), "\"Read\"");
        assert_eq!(serde_json::to_string(&DfAccessMode::Mutate).unwrap(), "\"Mutate\"");
        assert_eq!(serde_json::to_string(&DfAccessMode::Create).unwrap(), "\"Create\"");
        assert_eq!(serde_json::to_string(&DfAccessMode::Delete).unwrap(), "\"Delete\"");
    }
}
