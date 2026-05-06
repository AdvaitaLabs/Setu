//! SetuModuleResolver — bridges Setu storage and Move VM module loading.
//!
//! Implements `ModuleResolver` + `LinkageResolver` for Move VM sessions.
//! Loads stdlib from in-memory cache, user modules from `RawStore`.

use std::collections::HashMap;

use move_binary_format::errors::PartialVMError;
use move_core_types::{
    language_storage::ModuleId,
    resolver::{LinkageResolver, ModuleResolver},
    vm_status::StatusCode,
};

use setu_runtime::state::RawStore;

/// Module resolver bridging Setu storage to Move VM.
///
/// Lookup order:
/// 1. Precompiled stdlib cache (in-memory HashMap)
/// 2. User-deployed modules from RawStore (key: `"mod:{addr}::{name}"`)
pub struct SetuModuleResolver<'a, S: RawStore> {
    store: &'a S,
    stdlib_cache: &'a HashMap<ModuleId, Vec<u8>>,
}

impl<'a, S: RawStore> SetuModuleResolver<'a, S> {
    pub fn new(store: &'a S, stdlib_cache: &'a HashMap<ModuleId, Vec<u8>>) -> Self {
        Self {
            store,
            stdlib_cache,
        }
    }

    /// Build storage key for a module: `"mod:{hex_addr}::{module_name}"`
    fn module_key(module_id: &ModuleId) -> String {
        format!("mod:{}::{}", module_id.address().to_hex_literal(), module_id.name())
    }
}

impl<'a, S: RawStore> LinkageResolver for SetuModuleResolver<'a, S> {
    type Error = PartialVMError;
    // Use default implementations.
    //
    // Setu does NOT install a per-package linkage table at the LinkageResolver
    // level — the upstream Sui mechanism (resolve type/module via "linkage
    // context") is bypassed. Instead, B5's package upgrade strategy (Phase 8 /
    // design.md §4.5) bakes the new self-address directly into module
    // bytecode at upgrade time via [`crate::relink::relink_module`], so by
    // the time the VM reads a module out of storage its
    // `address_identifiers[0]` already points at the correct version. The
    // `linkage:latest:{family_id}` and `linkage:hist:{family_id}:{version}`
    // state-key entries written by `lower_publish_inline` / `lower_upgrade_inline`
    // are read by callers (RPC, `submit_move_call`) to discover *which*
    // package address to invoke; once the address is chosen, normal
    // ModuleResolver lookup is sufficient.
}

impl<'a, S: RawStore> ModuleResolver for SetuModuleResolver<'a, S> {
    type Error = PartialVMError;

    fn get_module(&self, module_id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        // 1. Check stdlib cache first
        if let Some(bytes) = self.stdlib_cache.get(module_id) {
            return Ok(Some(bytes.clone()));
        }

        // 2. Load user module from storage
        let key = Self::module_key(module_id);
        match self.store.get_raw(&key) {
            Ok(opt) => Ok(opt),
            Err(e) => Err(PartialVMError::new(StatusCode::STORAGE_ERROR)
                .with_message(format!("Failed to load module {}: {}", module_id, e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use move_core_types::{account_address::AccountAddress, identifier::Identifier};
    use setu_runtime::state::InMemoryObjectStore;

    #[test]
    fn test_resolver_stdlib_hit() {
        let store = InMemoryObjectStore::new();
        let mut stdlib = HashMap::new();
        let mid = ModuleId::new(AccountAddress::ONE, Identifier::new("test_mod").unwrap());
        stdlib.insert(mid.clone(), vec![0xCA, 0xFE]);

        let resolver = SetuModuleResolver::new(&store, &stdlib);
        let result = resolver.get_module(&mid).unwrap();
        assert_eq!(result, Some(vec![0xCA, 0xFE]));
    }

    #[test]
    fn test_resolver_miss() {
        let store = InMemoryObjectStore::new();
        let stdlib = HashMap::new();
        let mid = ModuleId::new(AccountAddress::ONE, Identifier::new("nonexistent").unwrap());

        let resolver = SetuModuleResolver::new(&store, &stdlib);
        let result = resolver.get_module(&mid).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolver_user_module() {
        let mut store = InMemoryObjectStore::new();
        let stdlib = HashMap::new();

        let mid = ModuleId::new(AccountAddress::ONE, Identifier::new("user_mod").unwrap());
        let key = SetuModuleResolver::<InMemoryObjectStore>::module_key(&mid);
        store.set_raw(&key, vec![0xDE, 0xAD]).unwrap();

        let resolver = SetuModuleResolver::new(&store, &stdlib);
        let result = resolver.get_module(&mid).unwrap();
        assert_eq!(result, Some(vec![0xDE, 0xAD]));
    }

    #[test]
    fn test_stdlib_takes_priority() {
        let mut store = InMemoryObjectStore::new();
        let mut stdlib = HashMap::new();

        let mid = ModuleId::new(AccountAddress::ONE, Identifier::new("overlap").unwrap());
        stdlib.insert(mid.clone(), vec![0x01]);

        let key = SetuModuleResolver::<InMemoryObjectStore>::module_key(&mid);
        store.set_raw(&key, vec![0x02]).unwrap();

        let resolver = SetuModuleResolver::new(&store, &stdlib);
        let result = resolver.get_module(&mid).unwrap();
        assert_eq!(result, Some(vec![0x01])); // stdlib wins
    }

    #[test]
    fn test_module_key_format() {
        let mid = ModuleId::new(
            AccountAddress::from_hex_literal("0x1").unwrap(),
            Identifier::new("object").unwrap(),
        );
        let key = SetuModuleResolver::<InMemoryObjectStore>::module_key(&mid);
        assert!(key.starts_with("mod:"));
        assert!(key.contains("::object"));
    }
}
