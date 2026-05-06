//! Infrastructure Event Executor
//!
//! Executes infrastructure events (SubnetRegister, UserRegister) directly in Validator.
//! These events are NOT routed to Solvers/TEE because they are core infrastructure
//! operations managed by validators.
//!
//! ## Architecture
//!
//! ```text
//! Infrastructure Events (Validator-executed):
//! - SubnetRegister: Create subnet, mint initial tokens
//! - UserRegister: Register user membership
//! - ValidatorRegister/Unregister
//! - SolverRegister/Unregister
//!
//! Application Events (Solver-executed via TEE):
//! - Transfer: Token transfers
//! - (Future) Smart contract calls
//! ```
//!
//! ## Design Philosophy
//!
//! Infrastructure primitives (subnet/user registration) are handled by Validator
//! to ensure consistency and avoid TEE complexity for non-economic operations.
//! Token operations (initial minting, airdrops) use the same RuntimeExecutor
//! logic as TEE to maintain consistency.

use setu_runtime::{RuntimeExecutor, ExecutionContext, InMemoryStateStore};
use setu_storage::MerkleStateProvider;
use setu_types::{
    Address,
    object::ObjectId,
    registration::{SubnetRegistration, UserRegistration},
    event::{Event, ExecutionResult, MoveUpgradePayload, StateChange as EventStateChange},
};
use setu_vlc::VLCSnapshot;
use std::sync::Arc;
use tracing::info;

/// Infrastructure event executor for Validator
///
/// Executes SubnetRegister, UserRegister events directly without TEE.
pub struct InfraExecutor {
    /// Validator ID
    validator_id: String,
    /// State provider for reading/writing state
    state_provider: Arc<MerkleStateProvider>,
}

impl InfraExecutor {
    /// Create a new infrastructure executor
    pub fn new(validator_id: String, state_provider: Arc<MerkleStateProvider>) -> Self {
        Self {
            validator_id,
            state_provider,
        }
    }

    /// Execute a SubnetRegister event
    ///
    /// This creates a subnet and optionally mints initial tokens to the owner.
    pub fn execute_subnet_register(
        &self,
        registration: &SubnetRegistration,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        // Derive tx_hash from registration for deterministic ID generation
        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:SUBNET:");
            hasher.update(registration.subnet_id.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(),
            timestamp,
            false,
            tx_hash,
        );

        // Create a temporary InMemoryStateStore for execution
        // In production, this would integrate with the actual state
        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

        let owner = Address::from_hex(&registration.owner)
            .map_err(|e| format!("Invalid owner address '{}': {}", registration.owner, e))?;

        let output = runtime.execute_subnet_register(
            &registration.subnet_id,
            &registration.name,
            &owner,
            registration.token_symbol.as_deref(),
            registration.initial_token_supply,
            &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Subnet registration failed".to_string()));
        }

        // Convert RuntimeExecutor output to Event
        let mut event = Event::subnet_register(
            registration.clone(),
            vec![], // No parent events
            vlc_snapshot,
            self.validator_id.clone(),
        );

        // Phase 5 (consensus-root-self-consistency): state changes are NOT
        // applied to the write-GSM here. The canonical write path is
        // `apply_committed_events` invoked from CF finalize. Eager-apply on
        // the ingress validator caused cross-node SMT divergence (OBS-026).
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();

        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(
            subnet_id = %registration.subnet_id,
            owner = %registration.owner,
            token_symbol = ?registration.token_symbol,
            initial_supply = ?registration.initial_token_supply,
            event_id = %event.id,
            "Subnet registered by Validator"
        );

        Ok(event)
    }

    /// Execute a UserRegister event
    ///
    /// This registers a user in a subnet (membership only, no automatic airdrop).
    pub fn execute_user_register(
        &self,
        registration: &UserRegistration,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        // Derive tx_hash from registration for deterministic ID generation
        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:USER:");
            hasher.update(registration.address.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(),
            timestamp,
            false,
            tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

        let user_address = Address::from_hex(&registration.address)
            .map_err(|e| format!("Invalid user address '{}': {}", registration.address, e))?;
        let subnet_id = registration.subnet_id.as_deref().unwrap_or("subnet-0");

        let output = runtime.execute_user_register(
            &user_address,
            subnet_id,
            &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "User registration failed".to_string()));
        }

        // Convert to Event
        let mut event = Event::user_register(
            registration.clone(),
            vec![],
            vlc_snapshot,
            self.validator_id.clone(),
        );

        // Phase 5: no eager apply — see execute_subnet_register note (OBS-026).
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();

        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(
            user = %registration.address,
            subnet_id = %subnet_id,
            event_id = %event.id,
            "User registered by Validator"
        );

        Ok(event)
    }

    // ========== Phase 4: Move VM Contract Publish ==========

    /// Execute a ContractPublish event (Move module deployment)
    ///
    /// Verifies Move bytecode and stores modules in ROOT subnet SMT.
    /// Follows the same pattern as execute_subnet_register():
    /// execute → eager apply state → set execution_result → return Event
    pub fn execute_contract_publish(
        &self,
        sender: &str,
        modules_bytes: &[Vec<u8>],
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        use move_binary_format::CompiledModule;
        use move_bytecode_verifier::verify_module_unmetered;

        if modules_bytes.is_empty() {
            return Err("Empty module list".into());
        }

        let mut state_changes: Vec<EventStateChange> = Vec::new();
        // B5: family_id is the bundle's self-address (must be uniform across
        // all modules in a single publish call — Move language invariant).
        let mut family_addr: Option<setu_types::object::Address> = None;

        for module_bytes in modules_bytes {
            // 1. Deserialize
            let compiled = CompiledModule::deserialize_with_defaults(module_bytes)
                .map_err(|e| format!("Module deserialization failed: {}", e))?;

            // 2. Bytecode verification
            verify_module_unmetered(&compiled)
                .map_err(|e| format!("Bytecode verification failed: {}", e))?;

            // 3. Build "mod:{addr}::{name}" key
            let module_addr = compiled.self_id().address().to_hex_literal();
            let module_name = compiled.self_id().name().to_string();
            let module_key = format!("mod:{}::{}", module_addr, module_name);

            // B5: assert all modules in the bundle share the same self-address
            // (= family_id at publish time). Mixed-address bundles are rejected
            // because they would split the family into multiple linkage roots.
            let self_addr_bytes = compiled.self_id().address().into_bytes();
            let self_addr = setu_types::object::Address::new(self_addr_bytes);
            match family_addr {
                None => family_addr = Some(self_addr),
                Some(prev) if prev != self_addr => {
                    return Err(format!(
                        "ContractPublish: heterogeneous self-addresses in bundle ({} vs {})",
                        hex::encode(prev.as_bytes()),
                        hex::encode(self_addr.as_bytes())
                    ));
                }
                _ => {}
            }

            // 4. ADR-4: reject duplicate publish — check on-chain state
            if self.state_provider.get_raw_data(&module_key).is_some() {
                return Err(format!("Module already published (ADR-4): {}", module_key));
            }

            // 5. In-batch duplicate check
            if state_changes.iter().any(|sc| sc.key == module_key) {
                return Err(format!("Duplicate module in batch: {}", module_key));
            }

            // 6. Build StateChange (key = "mod:{addr}::{name}", value = raw bytecode)
            state_changes.push(EventStateChange::insert(
                module_key,
                module_bytes.clone(),
            ));
        }

        // B5: emit `linkage:latest:{family_hex}` = bcs((family_addr, 0u64)).
        // This is the v0 anchor every subsequent `execute_move_upgrade` /
        // `lower_upgrade_inline` storage-probe relies on. Schema is locked
        // to match the mock TEE writer at
        // `crates/setu-enclave/src/mock/mod.rs` (PublishWithLinkage arm) and
        // engine PTB path — keep BCS layout `(Address, u64)` invariant.
        if let Some(addr) = family_addr {
            let family_hex = hex::encode(addr.as_bytes());
            let linkage_key = format!("linkage:latest:{}", family_hex);
            // Skip emit if a linkage entry already exists (idempotent across
            // a re-published family root — should not happen because mod
            // dedupe at step 4 already rejected; defensive only).
            if self.state_provider.get_raw_data(&linkage_key).is_none()
                && !state_changes.iter().any(|sc| sc.key == linkage_key)
            {
                let payload = bcs::to_bytes(&(addr, 0u64))
                    .map_err(|e| format!("linkage:latest BCS encode: {}", e))?;
                state_changes.push(EventStateChange::insert(linkage_key, payload));
            }
        }

        // Phase 5 (consensus-root-self-consistency / OBS-026): NO eager apply.
        // The previous step 7 wrote directly to the ingress validator's
        // write-GSM, causing cross-node SMT divergence because non-ingress
        // validators only saw the write at CF finalize time. The canonical
        // write path is `apply_committed_events` at CF finalize — same path
        // used by follower validators. The ADR-4 duplicate-publish pre-check
        // above (step 4) now only rejects against *confirmed* (CF-finalized)
        // state; within-CF / concurrent duplicate publishes are caught by
        // the conflict resolver at apply time.

        // 8. Build Event
        let mut event = Event::contract_publish(
            sender.to_string(),
            modules_bytes.to_vec(),
            vec![], // No parent events
            vlc_snapshot,
            self.validator_id.clone(),
        );

        // 9. Set execution_result
        event.set_execution_result(ExecutionResult {
            success: true,
            message: Some(format!("{} module(s) published", state_changes.len())),
            state_changes,
        });

        info!(
            sender = %sender,
            module_count = modules_bytes.len(),
            event_id = %event.id,
            "Contract published by Validator"
        );

        Ok(event)
    }

    // ========== B5: Move Package Upgrade ==========

    /// Execute a Move package upgrade event (B5, β-1 fresh-address-per-version).
    ///
    /// **v0 contract**:
    /// - `current_package` is treated as both the family root AND the previous
    ///   package head (i.e. only v0 → v1 upgrades supported; v1 → v2 chained
    ///   upgrades require a `family_of:{addr}` reverse index, deferred).
    /// - Compatibility check (`compat::check_upgrade_compat`) is NOT
    ///   invoked yet — the helper exists but needs old-module bytecode
    ///   resolution + `Normalized::Module` construction (~80 LoC, deferred).
    /// - UpgradeCap minting + version-bump MoveCall is NOT performed
    ///   (parallels engine-path v0).
    /// - The upgrade derives `new_package_addr` deterministically as
    ///   `blake3("SETU_PKG_VER:" || family_id || new_version_le)` so
    ///   replays produce the same address.
    ///
    /// Emits state changes (NOT eager-applied per OBS-026):
    ///   - one `mod:{new_addr}::{name}` per relinked module
    ///   - one `linkage:latest:{family_hex}` = `bcs((new_addr, new_version))`
    ///
    /// Mirrors `engine::lower_upgrade_inline` — keep the address-derivation
    /// formula and BCS schema in sync.
    pub fn execute_move_upgrade(
        &self,
        sender: &str,
        current_package: ObjectId,
        modules_bytes: &[Vec<u8>],
        deps: Vec<ObjectId>,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        use move_binary_format::CompiledModule;
        use move_bytecode_verifier::verify_module_unmetered;
        use move_core_types::account_address::AccountAddress;
        use setu_move_vm::relink::relink_module;

        if modules_bytes.is_empty() {
            return Err("Empty module list".into());
        }

        // 1. Probe linkage:latest:{family_hex} (v0: family = current_package).
        let family_hex = hex::encode(current_package.as_bytes());
        let linkage_key = format!("linkage:latest:{}", family_hex);
        let linkage_bytes = self
            .state_provider
            .get_raw_data(&linkage_key)
            .ok_or_else(|| {
                format!(
                    "Move upgrade: no linkage entry for family={} \
                     (was the package published via execute_contract_publish?)",
                    family_hex
                )
            })?;
        let (prev_addr, prev_version): (setu_types::object::Address, u64) =
            bcs::from_bytes(&linkage_bytes)
                .map_err(|e| format!("Move upgrade: linkage:latest BCS decode: {}", e))?;

        // 2. Bump version, derive new package address.
        let new_version = prev_version
            .checked_add(1)
            .ok_or_else(|| "Move upgrade: version overflow".to_string())?;
        let new_addr_bytes = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_PKG_VER:");
            hasher.update(current_package.as_bytes());
            hasher.update(&new_version.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let new_addr = setu_types::object::Address::new(new_addr_bytes);
        let prev_account = AccountAddress::new(*prev_addr.as_bytes());
        let new_account = AccountAddress::new(new_addr_bytes);

        // 3. Per-module: deserialize → verify → relink → re-deserialize for name.
        let mut state_changes: Vec<EventStateChange> = Vec::new();
        let mut relinked_modules: Vec<Vec<u8>> = Vec::with_capacity(modules_bytes.len());

        for original_bytes in modules_bytes {
            let compiled = CompiledModule::deserialize_with_defaults(original_bytes)
                .map_err(|e| format!("Module deserialization failed: {}", e))?;
            verify_module_unmetered(&compiled)
                .map_err(|e| format!("Bytecode verification failed: {}", e))?;

            // Reject bundles whose self-address doesn't match prev_addr —
            // mirrors engine path (relink_module would silently no-op,
            // producing a bundle that publishes nothing useful).
            let self_addr_bytes = compiled.self_id().address().into_bytes();
            if &self_addr_bytes != prev_addr.as_bytes() {
                return Err(format!(
                    "Move upgrade: module self-address {} does not match \
                     prev_package {} (chained upgrades require family_of \
                     reverse index, not yet supported)",
                    hex::encode(self_addr_bytes),
                    hex::encode(prev_addr.as_bytes())
                ));
            }

            // Relink old_addr → new_addr.
            let (relinked, _stats) =
                relink_module(original_bytes, prev_account, new_account)
                    .map_err(|e| format!("relink module: {}", e))?;

            // Re-derive module name from relinked bytes.
            let relinked_module = CompiledModule::deserialize_with_defaults(&relinked)
                .map_err(|e| format!("Relinked module deserialize: {}", e))?;
            let module_name = relinked_module.self_id().name().to_string();
            let new_addr_hex = format!("0x{}", hex::encode(new_addr_bytes));
            let module_key = format!("mod:{}::{}", new_addr_hex, module_name);

            // Defensive duplicate check (fresh address ⇒ should always pass).
            if self.state_provider.get_raw_data(&module_key).is_some() {
                return Err(format!(
                    "Move upgrade: derived address collision (ADR-4): {}",
                    module_key
                ));
            }
            if state_changes.iter().any(|sc| sc.key == module_key) {
                return Err(format!("Move upgrade: duplicate module in bundle: {}", module_key));
            }

            state_changes.push(EventStateChange::insert(module_key, relinked.clone()));
            relinked_modules.push(relinked);
        }

        // 4. Emit linkage:latest:{family} = bcs((new_addr, new_version)).
        //
        // MUST use `update` (carries old_value) — at publish time we already
        // wrote bcs((family_addr, 0u64)) under this key, so the apply-path
        // R15 "create-where-key-exists" defence in
        // storage::state::manager::apply_committed_events would silently
        // skip the entire upgrade event if we used `insert` here, dropping
        // the sibling `mod:{new_addr}::{name}` writes too.
        // See docs/feat/fix-upgraded-module-not-visible/design.md.
        {
            let payload = bcs::to_bytes(&(new_addr, new_version))
                .map_err(|e| format!("Move upgrade: linkage:latest BCS encode: {}", e))?;
            state_changes.push(EventStateChange::update(linkage_key, linkage_bytes, payload));
        }

        // 5. Build payload + Event.
        let sender_addr = setu_types::Address::from_hex(sender)
            .map_err(|e| format!("Invalid sender address '{}': {}", sender, e))?;
        let digest = {
            // bcs(modules || deps) — see MoveUpgradePayload::digest doc.
            let mut bytes = Vec::new();
            for m in &relinked_modules {
                bytes.extend_from_slice(m);
            }
            for d in &deps {
                bytes.extend_from_slice(d.as_bytes());
            }
            bytes
        };
        let payload = MoveUpgradePayload {
            sender: sender_addr,
            family_id: current_package,
            prev_package: current_package,
            new_package_addr: new_addr,
            new_version,
            modules: relinked_modules,
            deps,
            digest,
            // v0: UpgradeCap not minted; reuse current_package as a placeholder
            // sentinel so the payload field stays valid for replay assertion.
            upgrade_cap_id: current_package,
            // 0 = Compatible (default policy until UpgradeCap minting lands).
            policy: 0,
        };
        let mut event = Event::move_upgrade(
            payload,
            vec![],
            vlc_snapshot,
            self.validator_id.clone(),
        );

        event.set_execution_result(ExecutionResult {
            success: true,
            message: Some(format!(
                "{} module(s) upgraded to v{} at {:?}",
                state_changes.len() - 1,
                new_version,
                new_addr.as_bytes()
            )),
            state_changes,
        });

        info!(
            sender = %sender,
            family = %family_hex,
            new_version,
            module_count = modules_bytes.len(),
            event_id = %event.id,
            "Move package upgraded by Validator"
        );

        Ok(event)
    }

    // Phase 5 (2026-04-24, consensus-root-self-consistency / OBS-026):
    // The `apply_state_changes` helper was removed. It was the shared
    // ingress-only eager-apply that caused cross-node SMT divergence:
    // only the ingress validator saw the state, followers only observed it
    // at CF finalize → base-state mismatch → `RootMismatch` → follower
    // "syncing metadata only" path permanently encoded the split. All six
    // infra executors (subnet_register, user_register, contract_publish,
    // profile_update, subnet_join, subnet_leave) now rely exclusively on
    // the canonical CF-apply path via `apply_committed_events`. Clients
    // needing read-your-writes for infra objects must wait for CF
    // confirmation (GET /api/v1/events/{id}) before querying state.

    // ========== Phase 3: Profile & Subnet Membership ==========

    /// Execute a profile update event
    pub fn execute_profile_update(
        &self,
        user_address: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        bio: Option<&str>,
        attributes: &std::collections::HashMap<String, String>,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:PROFILE:");
            hasher.update(user_address.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(), timestamp, false, tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);
        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;

        let output = runtime.execute_profile_update(
            &address, display_name, avatar_url, bio, attributes, &ctx,
        ).map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Profile update failed".to_string()));
        }

        let mut event = Event::new(
            setu_types::event::EventType::System, vec![], vlc_snapshot, self.validator_id.clone(),
        );

        // Phase 5: no eager apply — see execute_subnet_register note (OBS-026).
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(user = %user_address, event_id = %event.id, "Profile updated by Validator");
        Ok(event)
    }

    /// Execute a subnet join event
    pub fn execute_subnet_join(
        &self,
        user_address: &str,
        subnet_id: &str,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:JOIN:");
            hasher.update(user_address.as_bytes());
            hasher.update(subnet_id.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(), timestamp, false, tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);
        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;

        let output = runtime.execute_subnet_join(&address, subnet_id, &ctx)
            .map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Subnet join failed".to_string()));
        }

        let mut event = Event::new(
            setu_types::event::EventType::System, vec![], vlc_snapshot, self.validator_id.clone(),
        );

        // Phase 5: no eager apply — see execute_subnet_register note (OBS-026).
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(user = %user_address, subnet_id = %subnet_id, event_id = %event.id, "Subnet join by Validator");
        Ok(event)
    }

    /// Execute a subnet leave event
    pub fn execute_subnet_leave(
        &self,
        user_address: &str,
        subnet_id: &str,
        vlc_snapshot: VLCSnapshot,
    ) -> Result<Event, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;

        let tx_hash = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"SETU_TX_HASH:VALIDATOR:LEAVE:");
            hasher.update(user_address.as_bytes());
            hasher.update(subnet_id.as_bytes());
            hasher.update(&timestamp.to_le_bytes());
            *hasher.finalize().as_bytes()
        };
        let ctx = ExecutionContext::new(
            self.validator_id.clone(), timestamp, false, tx_hash,
        );

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);
        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;

        let output = runtime.execute_subnet_leave(&address, subnet_id, &ctx)
            .map_err(|e| format!("Runtime error: {}", e))?;

        if !output.success {
            return Err(output.message.unwrap_or_else(|| "Subnet leave failed".to_string()));
        }

        let mut event = Event::new(
            setu_types::event::EventType::System, vec![], vlc_snapshot, self.validator_id.clone(),
        );

        // Phase 5: no eager apply — see execute_subnet_register note (OBS-026).
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        event.set_execution_result(ExecutionResult {
            success: true,
            message: output.message,
            state_changes,
        });

        info!(user = %user_address, subnet_id = %subnet_id, event_id = %event.id, "Subnet leave by Validator");
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_storage::{GlobalStateManager, SharedStateManager};
    use setu_types::SubnetId;

    #[test]
    fn test_subnet_register() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = SubnetRegistration::new("subnet-test", "Test Subnet", "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf", "TEST")
            .with_initial_supply(1_000_000);

        let vlc = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        };

        let result = executor.execute_subnet_register(&registration, vlc);
        assert!(result.is_ok());
        
        let event = result.unwrap();
        assert!(event.execution_result.is_some());
        assert!(event.execution_result.as_ref().unwrap().success);
    }

    #[test]
    fn test_user_register() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = UserRegistration::from_metamask(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            1000,
        ).with_subnet("subnet-0");

        let vlc = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        };

        let result = executor.execute_user_register(&registration, vlc);
        assert!(result.is_ok());
        
        let event = result.unwrap();
        assert!(event.execution_result.is_some());
        assert!(event.execution_result.as_ref().unwrap().success);
    }

    fn test_vlc() -> VLCSnapshot {
        VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: 1,
            physical_time: 1000,
        }
    }

    #[test]
    fn test_infra_profile_update_produces_system_event() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);
        let attrs = std::collections::HashMap::new();

        let result = executor.execute_profile_update(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            Some("Alice"), None, Some("Hello world"), &attrs, test_vlc(),
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::System);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 1);
        // Key must be "oid:{hex}" format
        assert!(er.state_changes[0].key.starts_with("oid:"));
    }

    #[test]
    fn test_infra_subnet_join_produces_system_event() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_subnet_join(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "defi-subnet", test_vlc(),
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::System);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 2);
        assert!(er.state_changes[0].key.starts_with("oid:"));
        assert!(er.state_changes[1].key.starts_with("oid:"));
        // Both have new_value (Create)
        assert!(er.state_changes[0].new_value.is_some());
        assert!(er.state_changes[1].new_value.is_some());
    }

    #[test]
    fn test_infra_subnet_leave_produces_system_event() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_subnet_leave(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "defi-subnet", test_vlc(),
        );
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::System);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        assert_eq!(er.state_changes.len(), 2);
        // Delete: new_value = None
        assert!(er.state_changes[0].new_value.is_none());
        assert!(er.state_changes[1].new_value.is_none());
    }

    /// Helper: create a valid Move module with a given address and name
    fn make_module_bytes(addr: move_core_types::account_address::AccountAddress, name: &str) -> Vec<u8> {
        use move_binary_format::file_format::*;
        let mut module = empty_module();
        module.address_identifiers[0] = addr;
        module.identifiers[0] = move_core_types::identifier::Identifier::new(name).unwrap();
        let mut buf = Vec::new();
        module.serialize_with_version(move_binary_format::file_format_common::VERSION_MAX, &mut buf)
            .expect("serialize empty module");
        buf
    }

    #[test]
    fn test_execute_contract_publish_basic() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead")
            .expect("valid addr");
        let module_bytes = make_module_bytes(addr, "counter");

        let result = executor.execute_contract_publish("alice", &[module_bytes], test_vlc());
        assert!(result.is_ok());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::ContractPublish);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        // B5: publish now emits 2 state changes — `mod:{addr}::{name}` for the
        // module + `linkage:latest:{family_hex}` for the upgrade chain anchor.
        assert_eq!(er.state_changes.len(), 2);
        assert!(er.state_changes.iter().any(|sc| sc.key.starts_with("mod:")));
        assert!(er.state_changes.iter().any(|sc| sc.key.starts_with("linkage:latest:")));
    }

    #[test]
    fn test_execute_contract_publish_empty_modules() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_contract_publish("alice", &[], test_vlc());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Empty module list"));
    }

    #[test]
    fn test_execute_contract_publish_invalid_bytecode() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_contract_publish("alice", &[vec![0xFF, 0x00]], test_vlc());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("deserialization failed"));
    }

    #[test]
    fn test_execute_contract_publish_adr4_duplicate() {
        // Phase 5 update (OBS-026): execute_contract_publish no longer eagerly
        // applies state. The ADR-4 duplicate-publish pre-check only fires
        // against *CF-finalized* state. To exercise it, we manually inject
        // the module key into the write-GSM to simulate a finalized prior CF,
        // then attempt a second publish and assert the ADR-4 error.
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(Arc::clone(&shared)));
        let executor = InfraExecutor::new("validator-1".to_string(), Arc::clone(&provider));

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead")
            .expect("valid addr");
        let module_bytes = make_module_bytes(addr, "counter");

        // First publish succeeds (returns an event; no state written yet)
        let result1 = executor.execute_contract_publish("alice", &[module_bytes.clone()], test_vlc());
        assert!(result1.is_ok());

        // Simulate the first CF finalizing: write the module key directly.
        // In production this is done by `apply_committed_events` on every node.
        {
            let mut gsm = shared.lock_write();
            let module_key = format!("mod:{}::{}", addr.to_hex_literal(), "counter");
            let sc = EventStateChange::insert(module_key, module_bytes.clone());
            gsm.apply_state_change(SubnetId::ROOT, &sc);
            shared.publish_snapshot(&gsm);
        }

        // Second publish of same module now fails (ADR-4) against confirmed state
        let result2 = executor.execute_contract_publish("alice", &[module_bytes], test_vlc());
        assert!(result2.is_err());
        assert!(result2.unwrap_err().contains("ADR-4"));
    }

    #[test]
    fn test_execute_contract_publish_batch_duplicate() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead")
            .expect("valid addr");
        let module_bytes = make_module_bytes(addr, "counter");

        // Same module twice in one batch
        let result = executor.execute_contract_publish(
            "alice",
            &[module_bytes.clone(), module_bytes],
            test_vlc(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Duplicate module in batch"));
    }

    // ========== B5: Move Package Upgrade tests ==========

    #[test]
    fn test_execute_move_upgrade_without_linkage_rejected() {
        // No prior publish ⇒ linkage:latest entry missing ⇒ rejection.
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead")
            .expect("valid addr");
        let module_bytes = make_module_bytes(addr, "counter");
        let mut family_arr = [0u8; 32];
        family_arr[31] = 0xDE;
        family_arr[30] = 0xAD;
        let family = setu_types::object::ObjectId::new(family_arr);

        let result = executor.execute_move_upgrade(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            family,
            &[module_bytes],
            vec![],
            test_vlc(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("no linkage entry"), "wrong error: {err}");
    }

    #[test]
    fn test_execute_move_upgrade_empty_modules_rejected() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let result = executor.execute_move_upgrade(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            setu_types::object::ObjectId::new([0xCD; 32]),
            &[],
            vec![],
            test_vlc(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Empty module list"));
    }
}
