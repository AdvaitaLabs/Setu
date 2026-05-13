//! Address compatibility between Setu and Move VM.
//!
//! Both Setu `Address`/`ObjectId` and Move `AccountAddress` are [u8; 32].
//! Conversion is zero-copy at the byte level.

use move_core_types::account_address::AccountAddress;
use setu_types::object::{Address, ObjectId};

// ═══════════════════════════════════════════════════════════════
// Setu → Move
// ═══════════════════════════════════════════════════════════════

/// Convert Setu Address to Move AccountAddress.
pub fn setu_addr_to_move(addr: &Address) -> AccountAddress {
    AccountAddress::new(*addr.as_bytes())
}

/// Convert Setu ObjectId to Move AccountAddress.
pub fn object_id_to_move(id: &ObjectId) -> AccountAddress {
    AccountAddress::new(*id.as_bytes())
}

// ═══════════════════════════════════════════════════════════════
// Move → Setu
// ═══════════════════════════════════════════════════════════════

/// Convert Move AccountAddress to Setu Address.
pub fn move_addr_to_setu(addr: &AccountAddress) -> Address {
    let bytes: [u8; 32] = addr.into_bytes();
    Address::new(bytes)
}

/// Convert Move AccountAddress to Setu ObjectId.
pub fn move_addr_to_object_id(addr: &AccountAddress) -> ObjectId {
    let bytes: [u8; 32] = addr.into_bytes();
    ObjectId::new(bytes)
}

// ═══════════════════════════════════════════════════════════════
// Raw byte helpers (kept from Phase 0 for backward compatibility)
// ═══════════════════════════════════════════════════════════════

/// Convert Setu Address to raw 32 bytes.
pub fn setu_addr_to_bytes(addr: &Address) -> [u8; 32] {
    *addr.as_bytes()
}

/// Convert raw 32 bytes to Setu Address.
pub fn bytes_to_setu_addr(bytes: [u8; 32]) -> Address {
    Address::new(bytes)
}

/// Convert Setu ObjectId to raw 32 bytes.
pub fn object_id_to_bytes(id: &ObjectId) -> [u8; 32] {
    *id.as_bytes()
}

/// Convert raw 32 bytes to Setu ObjectId.
pub fn bytes_to_object_id(bytes: [u8; 32]) -> ObjectId {
    ObjectId::new(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addr_roundtrip() {
        let addr = Address::new([0x11; 32]);
        let bytes = setu_addr_to_bytes(&addr);
        let restored = bytes_to_setu_addr(bytes);
        assert_eq!(addr, restored);
    }

    #[test]
    fn test_object_id_roundtrip() {
        let id = ObjectId::new([42u8; 32]);
        let bytes = object_id_to_bytes(&id);
        let restored = bytes_to_object_id(bytes);
        assert_eq!(id, restored);
    }

    #[test]
    fn test_setu_addr_to_move_roundtrip() {
        let addr = Address::new([0xAA; 32]);
        let move_addr = setu_addr_to_move(&addr);
        let back = move_addr_to_setu(&move_addr);
        assert_eq!(addr, back);
    }

    #[test]
    fn test_object_id_to_move_roundtrip() {
        let id = ObjectId::new([0xBB; 32]);
        let move_addr = object_id_to_move(&id);
        let back = move_addr_to_object_id(&move_addr);
        assert_eq!(id, back);
    }

    #[test]
    fn test_move_account_address_one() {
        let move_one = AccountAddress::ONE;
        let setu = move_addr_to_setu(&move_one);
        // AccountAddress::ONE = [0,0,...,0,1]
        assert_eq!(setu.as_bytes()[31], 1);
        let back = setu_addr_to_move(&setu);
        assert_eq!(back, move_one);
    }
}
