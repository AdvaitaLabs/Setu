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

use setu_runtime::{RuntimeExecutor, ExecutionContext, InMemoryStateStore, StateStore};
use setu_storage::{MerkleStateProvider, StateProvider};
use setu_types::{
    Address,
    flux_state_object_id,
    hash_utils::setu_hash_with_domain,
    object::ObjectId,
    object_key,
    power_state_object_id,
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
        //
        // BUG-20260510 fix: SubnetRegister is a root event so its
        // `event.get_subnet_id()` is `SubnetId::ROOT`. Without an explicit
        // `target_subnet`, `apply_committed_events` would write the minted
        // coin into the ROOT SMT while `get_coins_for_address` resolves the
        // coin's `coin_type` (= the new subnet id) via `resolve_subnet_id`
        // and looks in the app subnet SMT — silently dropping the balance.
        // We mark every state-change for an object in `output.created_objects`
        // (the mint coin id; subnet-meta is intentionally not in
        // `created_objects` per current runtime code) with the new subnet
        // as `target_subnet`, using the same canonical mapping the read
        // path uses. The subnet-meta state-change keeps `target_subnet =
        // None` so it lands in ROOT (the global subnet registry).
        let mint_object_ids: std::collections::HashSet<ObjectId> =
            output.created_objects.iter().copied().collect();
        let new_subnet_target =
            MerkleStateProvider::resolve_subnet_id(&registration.subnet_id);
        let state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| {
                let mut esc = sc.to_event_state_change();
                if mint_object_ids.contains(&sc.object_id) {
                    esc.target_subnet = Some(new_subnet_target);
                }
                esc
            })
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

        let user_address = Address::from_hex(&registration.address)
            .map_err(|e| format!("Invalid user address '{}': {}", registration.address, e))?;
        let subnet_id = registration.subnet_id.as_deref().unwrap_or("subnet-0");

        let mut temp_store = InMemoryStateStore::new();
        let addr_str = user_address.to_string();
        let flux_oid = flux_state_object_id(&addr_str);
        if let Some(bytes) = self.state_provider.get_object(&flux_oid) {
            temp_store
                .set_raw_object(flux_oid, bytes)
                .map_err(|e| format!("Runtime state seed error: {}", e))?;
        }
        let power_oid = power_state_object_id(&addr_str);
        if let Some(bytes) = self.state_provider.get_object(&power_oid) {
            temp_store
                .set_raw_object(power_oid, bytes)
                .map_err(|e| format!("Runtime state seed error: {}", e))?;
        }
        let mut runtime = RuntimeExecutor::new(temp_store);

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
    /// - Compatibility check via `setu_move_vm::compat::check_upgrade_compat`
    ///   is invoked per-module (policy hard-coded to `Compatible` until
    ///   `MoveUpgradeRequest` carries an explicit policy field). Bundles
    ///   that introduce module names absent from the prev package are
    ///   rejected, mirroring `engine.rs::lower_upgrade_inline`.
    ///   See `docs/feat/fix-infra-executor-skips-compat-check/`.
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

            // ABI compatibility check — mirrors
            // `engine.rs::lower_upgrade_inline`. Without this, the legacy
            // /api/v1/move/upgrade endpoint would accept ABI-breaking
            // upgrades that the PTB-encoded equivalent rejects.
            // See docs/bugs/20260506-infra-executor-skips-compat-check.md.
            //
            // CRITICAL: pass the PRE-relink `compiled` module (self-addr ==
            // prev_addr by the guard above), NOT `relinked_module` (self-addr
            // == new_addr). Upstream `Compatibility::check` rejects on
            // address mismatch (move-binary-format/compatibility.rs L120),
            // so comparing prev_addr-vs-new_addr would fail every reflexive
            // upgrade. Sui handles this via `MovePackage::normalize` which
            // relinks all modules to a common original_package_id before
            // compat — we get the same effect by feeding compat the
            // pre-relink (still-prev-addressed) bytecode.
            let old_key = format!(
                "mod:{}::{}",
                prev_account.to_hex_literal(),
                module_name
            );
            let old_bytes = self.state_provider.get_raw_data(&old_key).ok_or_else(|| {
                format!(
                    "Move upgrade: module '{}' has no prior entry under \
                     prev_addr={} (introducing new modules via upgrade is \
                     rejected — publish them in a fresh package instead)",
                    module_name,
                    prev_account.to_hex_literal()
                )
            })?;
            let old_module = CompiledModule::deserialize_with_defaults(&old_bytes)
                .map_err(|e| {
                    format!(
                        "Move upgrade: prev module '{}' deserialize failed: {}",
                        module_name, e
                    )
                })?;
            // Policy hard-coded to Compatible (= upstream `full_check`). This
            // is the strictest policy; userspace clients wanting AdditiveOnly
            // / DepOnly must use the PTB upgrade path which carries policy
            // on the ticket. MUST stay in sync with `payload.policy = 0` below.
            let policy = setu_move_vm::compat::UpgradePolicy::Compatible;
            setu_move_vm::compat::check_upgrade_compat(&old_module, &compiled, policy)
                .map_err(|setu_move_vm::compat::CompatError::Incompatible { status, msg }| {
                    format!(
                        "Move upgrade: compat check failed for module '{}' \
                         (policy={:?}, status={:?}): {}",
                        module_name, policy, status, msg
                    )
                })?;

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
            // 0 = Compatible. MUST stay in sync with `UpgradePolicy::Compatible`
            // passed to `check_upgrade_compat` above.
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

        let address = Address::from_hex(user_address)
            .map_err(|e| format!("Invalid address '{}': {}", user_address, e))?;
        let profile_key = format!("profile:{}", address);
        let profile_object_id = ObjectId::new(setu_hash_with_domain(
            b"SETU_PROFILE:",
            profile_key.as_bytes(),
        ));
        let existing_profile = self.state_provider.get_object(&profile_object_id);

        let temp_store = InMemoryStateStore::new();
        let mut runtime = RuntimeExecutor::new(temp_store);

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
        let mut state_changes: Vec<EventStateChange> = output.state_changes.iter()
            .map(|sc| sc.to_event_state_change())
            .collect();
        if let Some(existing_profile_bytes) = existing_profile {
            if state_changes.len() != 1 {
                return Err(format!(
                    "Profile update expected 1 state change, got {}",
                    state_changes.len()
                ));
            }

            let expected_key = object_key(&profile_object_id);
            let state_change = &mut state_changes[0];
            if state_change.key != expected_key {
                return Err(format!(
                    "Profile update state key mismatch: expected {}, got {}",
                    expected_key, state_change.key
                ));
            }

            let existing_json: serde_json::Value = serde_json::from_slice(&existing_profile_bytes)
                .map_err(|e| format!("Existing profile state decode error: {}", e))?;
            let created_at = existing_json
                .get("created_at")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| "Existing profile state missing created_at".to_string())?;

            let new_profile_bytes = state_change
                .new_value
                .as_mut()
                .ok_or_else(|| "Profile update missing new profile state".to_string())?;
            let mut new_json: serde_json::Value = serde_json::from_slice(new_profile_bytes)
                .map_err(|e| format!("New profile state decode error: {}", e))?;
            new_json["created_at"] = serde_json::json!(created_at);
            *new_profile_bytes = serde_json::to_vec(&new_json)
                .map_err(|e| format!("Profile update state encode error: {}", e))?;
            state_change.old_value = Some(existing_profile_bytes);
        }
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

    /// BUG-20260510 regression: SubnetRegister with `initial_token_supply > 0`
    /// must produce a mint state-change whose `target_subnet` is the new
    /// subnet (matching `MerkleStateProvider::resolve_subnet_id` of the
    /// subnet's string id). Without this, `apply_committed_events` would
    /// route the coin into the ROOT SMT while the read path looks in the
    /// app subnet SMT, and `get_balance` returns 0.
    #[test]
    fn test_subnet_register_mint_targets_new_subnet() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let subnet_str = "infra-subnet-mint-target-test";
        let registration = SubnetRegistration::new(
            subnet_str,
            "Mint Target Test",
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "MTT",
        )
        .with_initial_supply(1_000_000);

        let event = executor
            .execute_subnet_register(&registration, test_vlc())
            .expect("subnet_register must succeed");

        let result = event
            .execution_result
            .as_ref()
            .expect("execution_result populated");
        assert!(result.success, "registration must succeed");

        let expected_target = MerkleStateProvider::resolve_subnet_id(subnet_str);
        assert_ne!(
            expected_target,
            SubnetId::ROOT,
            "test setup: app subnet must not collide with ROOT"
        );

        // Exactly one state-change must be routed to the new subnet (the mint).
        // Subnet-meta state-change must keep target_subnet = None (lands in ROOT).
        let mut mint_routed = 0usize;
        let mut meta_in_root = 0usize;
        for sc in &result.state_changes {
            match sc.target_subnet {
                Some(t) => {
                    assert_eq!(
                        t, expected_target,
                        "mint state-change must target the new subnet, not {:?}",
                        t
                    );
                    mint_routed += 1;
                }
                None => {
                    meta_in_root += 1;
                }
            }
        }
        assert_eq!(
            mint_routed, 1,
            "expected exactly one mint state-change routed to the new subnet, got {}",
            mint_routed
        );
        assert!(
            meta_in_root >= 1,
            "expected at least one ROOT-bound state-change (subnet metadata), got {}",
            meta_in_root
        );
    }

    /// BUG-20260510 negative case: when `initial_token_supply` is None or 0,
    /// no mint state-change is produced, and every state-change stays in
    /// ROOT (subnet metadata only).
    #[test]
    fn test_subnet_register_no_supply_keeps_all_in_root() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(shared));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);

        let registration = SubnetRegistration::new(
            "infra-subnet-no-supply-test",
            "No Supply Test",
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "NST",
        );

        let event = executor
            .execute_subnet_register(&registration, test_vlc())
            .expect("subnet_register must succeed");

        let result = event.execution_result.as_ref().unwrap();
        assert!(result.success);
        for sc in &result.state_changes {
            assert!(
                sc.target_subnet.is_none(),
                "no mint expected, all state-changes must keep target_subnet = None"
            );
        }
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
        assert!(er.state_changes[0].old_value.is_none());
    }

    #[test]
    fn test_infra_profile_update_uses_update_state_change_when_profile_exists() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(Arc::clone(&shared)));
        let executor = InfraExecutor::new("validator-1".to_string(), provider);
        let attrs = std::collections::HashMap::new();
        let address = "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf";

        let first_event = executor
            .execute_profile_update(address, Some("Alice"), None, Some("Hello"), &attrs, test_vlc())
            .expect("first profile update must create profile");
        let first_state_change = &first_event
            .execution_result
            .as_ref()
            .expect("first event must have result")
            .state_changes[0];
        let first_profile_bytes = first_state_change
            .new_value
            .clone()
            .expect("first update must contain new profile bytes");
        let first_profile_json: serde_json::Value = serde_json::from_slice(&first_profile_bytes)
            .expect("first profile bytes must be JSON");
        let first_created_at = first_profile_json["created_at"]
            .as_u64()
            .expect("first profile must contain created_at");

        {
            let mut state_manager = shared.lock_write();
            let summary = state_manager.apply_committed_events(&[first_event]);
            assert!(summary.conflicted_events.is_empty());
            shared.publish_snapshot(&state_manager);
        }

        let second_event = executor
            .execute_profile_update(address, Some("Bob"), None, Some("Updated"), &attrs, test_vlc())
            .expect("second profile update must update profile");
        let second_state_change = &second_event
            .execution_result
            .as_ref()
            .expect("second event must have result")
            .state_changes[0];

        assert_eq!(second_state_change.old_value.as_ref(), Some(&first_profile_bytes));
        let second_profile_bytes = second_state_change
            .new_value
            .as_ref()
            .expect("second update must contain new profile bytes");
        let second_profile_json: serde_json::Value = serde_json::from_slice(second_profile_bytes)
            .expect("second profile bytes must be JSON");
        assert_eq!(second_profile_json["display_name"], "Bob");
        assert_eq!(second_profile_json["bio"], "Updated");
        assert_eq!(second_profile_json["created_at"], first_created_at);
        assert!(second_profile_json["updated_at"].as_u64().is_some());
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

    /// Helper for B5-fix tests: seed prev `mod:` key + `linkage:latest:` so
    /// `execute_move_upgrade` can probe them. Mirrors the on-chain effect of
    /// a successful `execute_contract_publish` having been CF-finalized.
    fn seed_published_v0(
        shared: &Arc<SharedStateManager>,
        addr: move_core_types::account_address::AccountAddress,
        module_name: &str,
        module_bytes: &[u8],
    ) {
        use setu_types::SubnetId;
        let addr_bytes = addr.into_bytes();
        let setu_addr = setu_types::object::Address::new(addr_bytes);
        let mod_key = format!("mod:{}::{}", addr.to_hex_literal(), module_name);
        let linkage_key = format!("linkage:latest:{}", hex::encode(addr_bytes));
        let linkage_payload = bcs::to_bytes(&(setu_addr, 0u64)).expect("bcs encode");

        let mut gsm = shared.lock_write();
        let sc1 = EventStateChange::insert(mod_key, module_bytes.to_vec());
        let sc2 = EventStateChange::insert(linkage_key, linkage_payload);
        gsm.apply_state_change(SubnetId::ROOT, &sc1);
        gsm.apply_state_change(SubnetId::ROOT, &sc2);
        shared.publish_snapshot(&gsm);
    }

    /// T1 (fix-infra-compat): self-replace upgrade passes the new compat
    /// gate. Proves the inserted `check_upgrade_compat` call does not
    /// regress the happy path. Reflexive compat semantics are covered by
    /// `setu_move_vm::compat::tests::u_co1`.
    #[test]
    fn test_execute_move_upgrade_compat_check_passes_self_replace() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(Arc::clone(&shared)));
        let executor = InfraExecutor::new("validator-1".to_string(), Arc::clone(&provider));

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead")
            .expect("valid addr");
        let module_bytes = make_module_bytes(addr, "counter");
        seed_published_v0(&shared, addr, "counter", &module_bytes);

        let family = setu_types::object::ObjectId::new(addr.into_bytes());
        let result = executor.execute_move_upgrade(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            family,
            &[module_bytes],
            vec![],
            test_vlc(),
        );
        assert!(result.is_ok(), "self-replace should pass compat: {:?}", result.err());
        let event = result.unwrap();
        assert_eq!(event.event_type, setu_types::event::EventType::ContractPublish);
        let er = event.execution_result.as_ref().unwrap();
        assert!(er.success);
        // mod:{new}::counter + linkage:latest update (= 2 changes)
        assert_eq!(er.state_changes.len(), 2);
        assert!(er.state_changes.iter().any(|sc| sc.key.starts_with("mod:")));
        assert!(er.state_changes.iter().any(|sc| sc.key.starts_with("linkage:latest:")));
    }

    /// T2 (fix-infra-compat): bundle whose module name is absent from the
    /// prev package is rejected before reaching the compat check. Mirrors
    /// `engine.rs::lower_upgrade_inline`'s "new modules cannot be
    /// introduced via Upgrade" rejection.
    #[test]
    fn test_execute_move_upgrade_rejects_new_module_name() {
        let shared = Arc::new(SharedStateManager::new(GlobalStateManager::new()));
        let provider = Arc::new(MerkleStateProvider::new(Arc::clone(&shared)));
        let executor = InfraExecutor::new("validator-1".to_string(), Arc::clone(&provider));

        let addr = move_core_types::account_address::AccountAddress::from_hex_literal("0xdead")
            .expect("valid addr");
        // Prev package has only "counter".
        let counter_bytes = make_module_bytes(addr, "counter");
        seed_published_v0(&shared, addr, "counter", &counter_bytes);

        // Upgrade bundle introduces a new name "other" at the same self-addr.
        let other_bytes = make_module_bytes(addr, "other");
        let family = setu_types::object::ObjectId::new(addr.into_bytes());
        let result = executor.execute_move_upgrade(
            "0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            family,
            &[other_bytes],
            vec![],
            test_vlc(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("no prior entry under prev_addr"),
            "wrong error: {err}"
        );
    }
}
