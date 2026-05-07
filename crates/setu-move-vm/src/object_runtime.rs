//! SetuObjectRuntime — injected into NativeContextExtensions for Move sessions.
//!
//! Tracks object creates, transfers, mutations, freezes, and deletions
//! during a single Move transaction execution.

use std::collections::BTreeMap;

use better_any::{Tid, TidAble};
use indexmap::{IndexMap, IndexSet};
use move_core_types::{account_address::AccountAddress, language_storage::StructTag};
use move_vm_runtime::native_extensions::NativeExtensionMarker;

use setu_types::dynamic_field::DfAccessMode;
use setu_types::object::{Address, ObjectId};
use setu_types::task::ResolvedDynamicField;
use setu_types::Ownership;

// ═══════════════════════════════════════════════════════════════
// Input / Effect types
// ═══════════════════════════════════════════════════════════════

/// Pre-loaded object passed into a Move session.
pub struct InputObject {
    pub id: ObjectId,
    pub owner: Address,
    /// Original ownership category preserved from the on-disk envelope.
    /// Required so that `&mut` mutations of `Shared`/`Immutable`/`ObjectOwner`
    /// inputs do not silently demote the object back to `AddressOwner`
    /// when the executor rebuilds the envelope. See R4-ISSUE-2.
    pub ownership: Ownership,
    pub version: u64,
    /// Full ObjectEnvelope BCS (for old_state in StateChange)
    pub envelope_bytes: Vec<u8>,
    /// Inner Move struct BCS (for VM consumption)
    pub move_data: Vec<u8>,
    pub type_tag: StructTag,
}

impl InputObject {
    /// Construct an InputObject from an ObjectEnvelope.
    ///
    /// Parses the envelope's type_tag string into a StructTag.
    /// Returns error if parsing fails (malformed data, not a silent skip).
    pub fn from_envelope(
        id: &ObjectId,
        env: &setu_types::ObjectEnvelope,
    ) -> Result<Self, setu_runtime::error::RuntimeError> {
        use std::str::FromStr;
        let type_tag = StructTag::from_str(&env.type_tag).map_err(|e| {
            setu_runtime::error::RuntimeError::InvalidTransaction(format!(
                "Failed to parse type_tag '{}': {}",
                env.type_tag, e
            ))
        })?;
        Ok(Self {
            id: *id,
            owner: env.metadata.owner,
            ownership: env.metadata.ownership,
            version: env.metadata.version,
            envelope_bytes: env.to_bytes(),
            move_data: env.data.clone(),
            type_tag,
        })
    }
}

/// Transfer effect recorded by `native_transfer_internal`.
pub struct ObjectTransferEffect {
    pub new_owner: AccountAddress,
    pub type_tag: StructTag,
    pub bcs_bytes: Vec<u8>,
}

/// Freeze effect recorded by `native_freeze_internal`.
pub struct ObjectFreezeEffect {
    pub type_tag: StructTag,
    pub bcs_bytes: Vec<u8>,
}

/// Share effect recorded by `native_share_internal` (PWOO).
///
/// Ownership of the object transitions from `AddressOwner` to
/// `Shared { initial_shared_version }`.
/// The `initial_shared_version` captures the envelope version at the
/// moment of share: for objects loaded from input_objects it is their
/// current version; for newly-created + same-TX share it is 0 (the
/// envelope is persisted with version bumped to 1 by the executor).
pub struct ObjectShareEffect {
    pub type_tag: StructTag,
    pub bcs_bytes: Vec<u8>,
    pub initial_shared_version: u64,
}

/// Mutation effect recorded for `&mut` objects.
pub struct ObjectMutationEffect {
    pub type_tag: StructTag,
    pub bcs_bytes: Vec<u8>,
}

// ═══════════════════════════════════════════════════════════════
// Dynamic Field runtime types (M2)
// ═══════════════════════════════════════════════════════════════

/// One DF entry pre-loaded into the runtime.
///
/// `value_bytes == None` ⇔ Create mode (no on-disk envelope yet).
/// `value_bytes == Some(bcs)` ⇔ Read / Mutate / Delete (existing entry).
///
/// `envelope_bytes` carries the full on-disk `ObjectEnvelope::to_bytes()` so
/// the engine's state-change converter can emit `old_state` that byte-matches
/// the current SMT — required to avoid false stale_read at CF apply.
pub(crate) struct DfEntry {
    pub df_oid: ObjectId,
    pub value_bytes: Option<Vec<u8>>,
    pub value_type_tag: String,
    pub mode: DfAccessMode,
    pub envelope_bytes: Option<Vec<u8>>,
}

/// Effect produced by `dynamic_field::add_internal` — a brand-new DF entry.
///
/// Distinct from `ObjectMutationEffect` because M3 needs `parent` + key info
/// to build the `ObjectEnvelope` with `Ownership::ObjectOwner(parent)` and
/// `data = bcs(DfFieldValue { parent, name_type_tag, name_bcs, value_bcs })`.
pub struct DfCreateEffect {
    pub parent: ObjectId,
    pub key_type_tag: String,
    pub key_bcs: Vec<u8>,
    pub value_type_tag: String,
    pub value_bcs: Vec<u8>,
}

/// Effect produced by `dynamic_field::borrow_mut_internal`.
///
/// M3 carries both old/new bytes so the converter can emit an Update
/// StateChange with the proper old envelope for byte-level conflict
/// detection. In M3 `new_value_bcs == old_value_bcs` (placeholder) until
/// full reference-tracking is added in a later milestone.
pub struct DfMutateEffect {
    pub parent: ObjectId,
    pub key_type_tag: String,
    pub key_bcs: Vec<u8>,
    pub value_type_tag: String,
    pub old_value_bcs: Vec<u8>,
    pub new_value_bcs: Vec<u8>,
    /// On-disk `ObjectEnvelope::to_bytes()` for this DF entry at tx-prepare time.
    /// When present, the engine uses it verbatim as `StateChange.old_value`
    /// (matching current SMT byte-for-byte) rather than rebuilding an envelope
    /// with an out-of-date version.
    pub on_disk_envelope: Option<Vec<u8>>,
}

/// Effect produced by `dynamic_field::remove_internal`.
///
/// Carries the original `value_bcs` so the converter can reconstruct the
/// old envelope for byte-level conflict detection.
pub struct DfDeleteEffect {
    pub parent: ObjectId,
    pub key_type_tag: String,
    pub key_bcs: Vec<u8>,
    pub value_type_tag: String,
    pub old_value_bcs: Vec<u8>,
    /// On-disk `ObjectEnvelope::to_bytes()` for this DF entry at tx-prepare time.
    /// Used by the engine for `StateChange.old_value` (see `DfMutateEffect`).
    pub on_disk_envelope: Option<Vec<u8>>,
}

/// Cache key: `(parent_oid, key_type_tag, key_bcs)`.
/// Same shape on both the preload side and the native lookup side.
pub(crate) type DfCacheKey = (ObjectId, String, Vec<u8>);

/// Aggregated results after a session finishes.
pub struct ObjectRuntimeResults {
    pub input_objects: BTreeMap<ObjectId, InputObject>,
    pub created_ids: IndexSet<ObjectId>,
    pub deleted_ids: IndexSet<ObjectId>,
    pub transfers: IndexMap<ObjectId, ObjectTransferEffect>,
    pub frozen: IndexMap<ObjectId, ObjectFreezeEffect>,
    pub shared: IndexMap<ObjectId, ObjectShareEffect>,
    pub mutated: IndexMap<ObjectId, ObjectMutationEffect>,
    pub emitted_events: Vec<(StructTag, Vec<u8>)>,
    // --- M2 Dynamic Field effects ---
    /// DF entries newly added by `dynamic_field::add` (ordered by first insert).
    pub df_created: IndexMap<ObjectId, DfCreateEffect>,
    /// DF entries mutated via `borrow_mut` (M3: placeholder old==new value_bcs;
    /// full reference-tracking deferred).
    pub df_mutated: IndexMap<ObjectId, DfMutateEffect>,
    /// DF entries removed via `dynamic_field::remove`.
    pub df_deleted: IndexMap<ObjectId, DfDeleteEffect>,
}

// ═══════════════════════════════════════════════════════════════
// SetuObjectRuntime
// ═══════════════════════════════════════════════════════════════

/// Runtime object tracker — injected as NativeContextExtension.
///
/// Natives access this via `context.extensions_mut().get_mut::<SetuObjectRuntime>()`.
/// Lifetime: one session execution.
#[derive(Tid)]
pub struct SetuObjectRuntime {
    // Input
    input_objects: BTreeMap<ObjectId, InputObject>,
    // Accumulated effects
    pub(crate) created_ids: IndexSet<ObjectId>,
    deleted_ids: IndexSet<ObjectId>,
    transfers: IndexMap<ObjectId, ObjectTransferEffect>,
    frozen: IndexMap<ObjectId, ObjectFreezeEffect>,
    shared: IndexMap<ObjectId, ObjectShareEffect>,
    mutated: IndexMap<ObjectId, ObjectMutationEffect>,
    pub(crate) emitted_events: Vec<(StructTag, Vec<u8>)>,
    // --- M2 Dynamic Field state ---
    df_cache: BTreeMap<DfCacheKey, DfEntry>,
    df_created: IndexMap<ObjectId, DfCreateEffect>,
    df_mutated: IndexMap<ObjectId, DfMutateEffect>,
    df_deleted: IndexMap<ObjectId, DfDeleteEffect>,
    // Context
    tx_hash: [u8; 32],
    ids_created: u64,
    sender: Address,
    epoch_timestamp_ms: u64,
}

impl<'a> NativeExtensionMarker<'a> for SetuObjectRuntime {}

impl SetuObjectRuntime {
    pub fn new(
        input_objects: Vec<InputObject>,
        df_preload: Vec<ResolvedDynamicField>,
        tx_hash: [u8; 32],
        sender: Address,
        epoch_timestamp_ms: u64,
    ) -> Self {
        let input_map = input_objects.into_iter().map(|obj| (obj.id, obj)).collect();
        let df_cache: BTreeMap<DfCacheKey, DfEntry> = df_preload
            .into_iter()
            .map(|rdf| {
                let key = (
                    rdf.parent_object_id,
                    rdf.name_type_tag.clone(),
                    rdf.name_bcs.clone(),
                );
                let entry = DfEntry {
                    df_oid: rdf.df_object_id,
                    value_bytes: rdf.value_bytes,
                    value_type_tag: rdf.value_type_tag,
                    mode: rdf.mode,
                    envelope_bytes: rdf.envelope_bytes,
                };
                (key, entry)
            })
            .collect();
        Self {
            input_objects: input_map,
            created_ids: IndexSet::new(),
            deleted_ids: IndexSet::new(),
            transfers: IndexMap::new(),
            frozen: IndexMap::new(),
            shared: IndexMap::new(),
            mutated: IndexMap::new(),
            emitted_events: Vec::new(),
            df_cache,
            df_created: IndexMap::new(),
            df_mutated: IndexMap::new(),
            df_deleted: IndexMap::new(),
            tx_hash,
            ids_created: 0,
            sender,
            epoch_timestamp_ms,
        }
    }

    /// Generate a deterministic object ID.
    /// ID = BLAKE3("SETU_FRESH_ID:" || tx_hash || ids_created.to_le_bytes())
    pub fn fresh_id(&mut self) -> ObjectId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SETU_FRESH_ID:");
        hasher.update(&self.tx_hash);
        hasher.update(&self.ids_created.to_le_bytes());
        let hash = hasher.finalize();
        self.ids_created += 1;

        let id = ObjectId::new(*hash.as_bytes());
        self.created_ids.insert(id);
        id
    }

    /// Record an object transfer.
    pub fn transfer_object(
        &mut self,
        id: ObjectId,
        new_owner: AccountAddress,
        type_tag: StructTag,
        bcs_bytes: Vec<u8>,
    ) {
        self.transfers.insert(
            id,
            ObjectTransferEffect {
                new_owner,
                type_tag,
                bcs_bytes,
            },
        );
    }

    /// Record an object mutation.
    pub fn mutate_object(&mut self, id: ObjectId, type_tag: StructTag, bcs_bytes: Vec<u8>) {
        self.mutated.insert(
            id,
            ObjectMutationEffect {
                type_tag,
                bcs_bytes,
            },
        );
    }

    /// Record an object freeze (Ownership → Immutable).
    pub fn freeze_object(&mut self, id: ObjectId, type_tag: StructTag, bcs_bytes: Vec<u8>) {
        self.frozen.insert(
            id,
            ObjectFreezeEffect {
                type_tag,
                bcs_bytes,
            },
        );
    }

    /// Record an object share (PWOO: Ownership → Shared).
    ///
    /// The `initial_shared_version` is captured from `input_objects` if the
    /// object was loaded as input; otherwise it defaults to 0 (same-TX
    /// newly-created + shared). Callers should have previously removed any
    /// conflicting transfer effect for the same id.
    pub fn share_object(&mut self, id: ObjectId, type_tag: StructTag, bcs_bytes: Vec<u8>) {
        let initial_shared_version = self
            .input_objects
            .get(&id)
            .map(|io| io.version)
            .unwrap_or(0);
        // `share` is terminal for ownership: drop any prior transfer effect
        // for the same object so downstream effect application sees Shared only.
        self.transfers.shift_remove(&id);
        self.shared.insert(
            id,
            ObjectShareEffect {
                type_tag,
                bcs_bytes,
                initial_shared_version,
            },
        );
    }

    /// Record an object deletion.
    pub fn delete_object(&mut self, id: ObjectId) {
        self.deleted_ids.insert(id);
    }

    /// Record a structured event emission.
    pub fn emit_event(&mut self, type_tag: StructTag, bcs_bytes: Vec<u8>) {
        self.emitted_events.push((type_tag, bcs_bytes));
    }

    /// Epoch timestamp in milliseconds (injected by consensus).
    pub fn epoch_timestamp_ms(&self) -> u64 {
        self.epoch_timestamp_ms
    }

    /// Look up a pre-loaded input object.
    pub fn get_input_object(&self, id: &ObjectId) -> Option<&InputObject> {
        self.input_objects.get(id)
    }

    /// Sender address for this transaction.
    pub fn sender(&self) -> &Address {
        &self.sender
    }

    // ─── M2 Dynamic Field helpers (native-facing) ───

    /// Check whether `id` is registered as an input object.
    /// Used by DF natives (v1.3 R3-ISSUE-4) to reject df_oids that collide
    /// with already-loaded input objects — a defence against a malicious
    /// `V: key` attempting to impersonate an input object.
    pub(crate) fn input_object_exists(&self, id: &ObjectId) -> bool {
        self.input_objects.contains_key(id)
    }

    /// Read-only cache lookup for DF natives.
    pub(crate) fn df_cache_get(&self, key: &DfCacheKey) -> Option<&DfEntry> {
        self.df_cache.get(key)
    }

    /// Mutable cache lookup for DF natives that need to update cached value
    /// bytes in-place after a `remove` (prevents a later `borrow` in the same
    /// tx from seeing stale bytes). M2 uses this only from remove/add.
    pub(crate) fn df_cache_get_mut(&mut self, key: &DfCacheKey) -> Option<&mut DfEntry> {
        self.df_cache.get_mut(key)
    }

    /// Record a DF create (from `dynamic_field::add_internal`).
    pub(crate) fn record_df_create(&mut self, df_oid: ObjectId, effect: DfCreateEffect) {
        self.df_created.insert(df_oid, effect);
    }

    /// Record a DF mutation (from `dynamic_field::borrow_mut_internal`).
    /// M3 stores old_value_bcs = new_value_bcs until ref-tracking is added.
    pub(crate) fn record_df_mutate(&mut self, df_oid: ObjectId, effect: DfMutateEffect) {
        self.df_mutated.insert(df_oid, effect);
    }

    /// Record a DF delete (from `dynamic_field::remove_internal`).
    ///
    /// Fold rule (fix-df-mutate-delete-same-oid, 2026-04-27):
    /// if `df_mutated[df_oid]` is already populated, the mutate never
    /// persisted to the SMT — collapse the pair into a single Delete using
    /// the PRE-TX `old_value_bcs` from the prior mutate. This handles two
    /// real same-tx sequences:
    ///
    /// 1. `remove → add → remove` (the second remove sees the in-memory
    ///    post-add value but the SMT still holds the pre-tx value).
    /// 2. `borrow_mut → remove`   (borrow_mut leaves a placeholder mutate
    ///    effect; the subsequent remove must collapse it).
    ///
    /// `effect.on_disk_envelope` is preserved as-is — it equals
    /// `prior.on_disk_envelope` because `df_cache_get_mut` only touches
    /// `value_bytes`, never `envelope_bytes`, during mutate-replace.
    pub(crate) fn record_df_delete(&mut self, df_oid: ObjectId, mut effect: DfDeleteEffect) {
        if let Some(prior) = self.df_mutated.swap_remove(&df_oid) {
            effect.old_value_bcs = prior.old_value_bcs;
        }
        self.df_deleted.insert(df_oid, effect);
    }

    /// Take and remove a previously-recorded DF delete effect.
    ///
    /// Used by mutate-replace flows (`remove` then `add` on the same key in
    /// one transaction) to collapse create+delete into a single update effect.
    pub(crate) fn take_df_delete(&mut self, df_oid: &ObjectId) -> Option<DfDeleteEffect> {
        self.df_deleted.swap_remove(df_oid)
    }

    /// Consume self and return aggregated results.
    pub fn into_results(self) -> ObjectRuntimeResults {
        ObjectRuntimeResults {
            input_objects: self.input_objects,
            created_ids: self.created_ids,
            deleted_ids: self.deleted_ids,
            transfers: self.transfers,
            frozen: self.frozen,
            shared: self.shared,
            mutated: self.mutated,
            emitted_events: self.emitted_events,
            df_created: self.df_created,
            df_mutated: self.df_mutated,
            df_deleted: self.df_deleted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_address() -> Address {
        Address::new([0xAA; 32])
    }

    fn test_tx_hash() -> [u8; 32] {
        [0xBB; 32]
    }

    #[test]
    fn test_fresh_id_deterministic() {
        let tx = test_tx_hash();
        let mut rt1 = SetuObjectRuntime::new(vec![], vec![], tx, test_address(), 0);
        let mut rt2 = SetuObjectRuntime::new(vec![], vec![], tx, test_address(), 0);

        let id1a = rt1.fresh_id();
        let id1b = rt1.fresh_id();
        let id2a = rt2.fresh_id();
        let id2b = rt2.fresh_id();

        assert_eq!(id1a, id2a, "same tx_hash + counter → same ID");
        assert_eq!(id1b, id2b);
        assert_ne!(id1a, id1b, "different counters → different IDs");
    }

    #[test]
    fn test_fresh_id_tracked_as_created() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let id = rt.fresh_id();
        assert!(rt.created_ids.contains(&id));
    }

    #[test]
    fn test_transfer_object() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let id = ObjectId::new([0x01; 32]);
        let recipient = AccountAddress::new([0x02; 32]);
        let tag = StructTag {
            address: AccountAddress::ONE,
            module: move_core_types::identifier::Identifier::new("coin").unwrap(),
            name: move_core_types::identifier::Identifier::new("Coin").unwrap(),
            type_params: vec![],
        };
        rt.transfer_object(id, recipient, tag, vec![0xDE, 0xAD]);

        let results = rt.into_results();
        assert_eq!(results.transfers.len(), 1);
        assert_eq!(results.transfers[&id].new_owner, recipient);
    }

    #[test]
    fn test_delete_created_is_noop() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let id = rt.fresh_id();
        assert!(rt.created_ids.contains(&id));

        // Deleting a freshly created object removes from created_ids
        // (mimics native_object_delete_uid behavior — tested externally)
        rt.created_ids.swap_remove(&id);
        assert!(!rt.created_ids.contains(&id));
    }

    #[test]
    fn test_into_results() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let _ = rt.fresh_id();
        rt.delete_object(ObjectId::new([0xFF; 32]));

        let results = rt.into_results();
        assert_eq!(results.created_ids.len(), 1);
        assert_eq!(results.deleted_ids.len(), 1);
    }

    fn test_struct_tag(name: &str) -> StructTag {
        StructTag {
            address: AccountAddress::ONE,
            module: move_core_types::identifier::Identifier::new("test").unwrap(),
            name: move_core_types::identifier::Identifier::new(name).unwrap(),
            type_params: vec![],
        }
    }

    #[test]
    fn test_emit_event_recorded() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let tag = test_struct_tag("MyEvent");
        rt.emit_event(tag.clone(), vec![0x01, 0x02]);
        assert_eq!(rt.emitted_events.len(), 1);
        assert_eq!(rt.emitted_events[0].0, tag);
        assert_eq!(rt.emitted_events[0].1, vec![0x01, 0x02]);
    }

    #[test]
    fn test_emit_multiple_events_ordered() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let tag_a = test_struct_tag("EventA");
        let tag_b = test_struct_tag("EventB");
        rt.emit_event(tag_a.clone(), vec![0xAA]);
        rt.emit_event(tag_b.clone(), vec![0xBB]);
        rt.emit_event(tag_a.clone(), vec![0xCC]);

        assert_eq!(rt.emitted_events.len(), 3);
        assert_eq!(rt.emitted_events[0].0, tag_a);
        assert_eq!(rt.emitted_events[1].0, tag_b);
        assert_eq!(rt.emitted_events[2].0, tag_a);
    }

    #[test]
    fn test_emit_events_in_results() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        rt.emit_event(test_struct_tag("Ev"), vec![0x42]);
        let results = rt.into_results();
        assert_eq!(results.emitted_events.len(), 1);
        assert_eq!(results.emitted_events[0].1, vec![0x42]);
    }

    #[test]
    fn test_epoch_timestamp_stored() {
        let rt = SetuObjectRuntime::new(
            vec![],
            vec![],
            test_tx_hash(),
            test_address(),
            1712700000000,
        );
        assert_eq!(rt.epoch_timestamp_ms(), 1712700000000);
    }

    #[test]
    fn test_epoch_timestamp_zero() {
        let rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        assert_eq!(rt.epoch_timestamp_ms(), 0);
    }

    // ═══════════════════════════════════════════════════════════════
    // PWOO: share_object tests
    // ═══════════════════════════════════════════════════════════════

    fn make_input_object(id: ObjectId, version: u64) -> InputObject {
        InputObject {
            id,
            owner: test_address(),
            ownership: Ownership::AddressOwner(test_address()),
            version,
            envelope_bytes: vec![],
            move_data: vec![],
            type_tag: test_struct_tag("Pool"),
        }
    }

    #[test]
    fn test_share_object_records_effect_for_newly_created() {
        // A3: newly-created + same-TX share → initial_shared_version = 0
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let id = rt.fresh_id();
        let tag = test_struct_tag("Pool");
        rt.share_object(id, tag.clone(), vec![0xAB, 0xCD]);

        let results = rt.into_results();
        assert_eq!(results.shared.len(), 1);
        let effect = &results.shared[&id];
        assert_eq!(effect.initial_shared_version, 0);
        assert_eq!(effect.type_tag, tag);
        assert_eq!(effect.bcs_bytes, vec![0xAB, 0xCD]);
    }

    #[test]
    fn test_share_object_preserves_input_version() {
        // A2: share of an input object captures its current version
        let id = ObjectId::new([0x77; 32]);
        let input = make_input_object(id, 5);
        let mut rt = SetuObjectRuntime::new(vec![input], vec![], test_tx_hash(), test_address(), 0);
        rt.share_object(id, test_struct_tag("Pool"), vec![0x01]);

        let results = rt.into_results();
        assert_eq!(results.shared[&id].initial_shared_version, 5);
    }

    #[test]
    fn test_share_object_zero_when_not_input() {
        // A3 (explicit): not in input_objects → version = 0 (synthetic)
        let id = ObjectId::new([0x99; 32]);
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        rt.share_object(id, test_struct_tag("Pool"), vec![]);

        let results = rt.into_results();
        assert_eq!(results.shared[&id].initial_shared_version, 0);
    }

    #[test]
    fn test_share_removes_prior_transfer_effect() {
        // A4: share is terminal — if transfer was recorded first, share wins
        let id = ObjectId::new([0x55; 32]);
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let tag = test_struct_tag("Pool");
        rt.transfer_object(id, AccountAddress::new([0x11; 32]), tag.clone(), vec![0xAA]);
        rt.share_object(id, tag, vec![0xBB]);

        let results = rt.into_results();
        assert!(!results.transfers.contains_key(&id));
        assert!(results.shared.contains_key(&id));
        assert_eq!(results.shared[&id].bcs_bytes, vec![0xBB]);
    }

    #[test]
    fn test_share_and_freeze_are_independent_maps() {
        // Defense-in-depth: share and freeze are recorded separately
        let id1 = ObjectId::new([0xA1; 32]);
        let id2 = ObjectId::new([0xA2; 32]);
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        rt.share_object(id1, test_struct_tag("P"), vec![]);
        rt.freeze_object(id2, test_struct_tag("F"), vec![]);
        let r = rt.into_results();
        assert_eq!(r.shared.len(), 1);
        assert_eq!(r.frozen.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // M2 Dynamic Field tests
    // ═══════════════════════════════════════════════════════════════

    fn rdf(
        parent: ObjectId,
        name_tag: &str,
        name_bcs: Vec<u8>,
        value_bytes: Option<Vec<u8>>,
        value_tag: &str,
        mode: DfAccessMode,
    ) -> ResolvedDynamicField {
        let df_oid = setu_types::dynamic_field::derive_df_oid(&parent, name_tag, &name_bcs);
        ResolvedDynamicField {
            parent_object_id: parent,
            df_object_id: df_oid,
            name_type_tag: name_tag.to_string(),
            name_bcs,
            value_bytes,
            value_type_tag: value_tag.to_string(),
            mode,
            envelope_bytes: None,
        }
    }

    #[test]
    fn test_new_with_df_preload_builds_cache() {
        // U1: preload 3 entries (1 Create + 2 Read) → cache populated correctly.
        let p = ObjectId::new([0x10; 32]);
        let preload = vec![
            rdf(
                p,
                "u64",
                vec![0, 0, 0, 0, 0, 0, 0, 1],
                None,
                "u64",
                DfAccessMode::Create,
            ),
            rdf(
                p,
                "u64",
                vec![0, 0, 0, 0, 0, 0, 0, 2],
                Some(vec![0xAA]),
                "u64",
                DfAccessMode::Read,
            ),
            rdf(
                p,
                "u64",
                vec![0, 0, 0, 0, 0, 0, 0, 3],
                Some(vec![0xBB]),
                "u64",
                DfAccessMode::Mutate,
            ),
        ];
        let rt = SetuObjectRuntime::new(vec![], preload, test_tx_hash(), test_address(), 0);

        let k1 = (p, "u64".to_string(), vec![0, 0, 0, 0, 0, 0, 0, 1]);
        let k2 = (p, "u64".to_string(), vec![0, 0, 0, 0, 0, 0, 0, 2]);
        let k3 = (p, "u64".to_string(), vec![0, 0, 0, 0, 0, 0, 0, 3]);

        assert!(rt.df_cache_get(&k1).unwrap().value_bytes.is_none());
        assert_eq!(rt.df_cache_get(&k2).unwrap().value_bytes, Some(vec![0xAA]));
        assert_eq!(rt.df_cache_get(&k3).unwrap().value_bytes, Some(vec![0xBB]));
    }

    #[test]
    fn test_df_cache_get_hit_and_miss() {
        // U2: present key → Some; absent key → None.
        let p = ObjectId::new([0x11; 32]);
        let preload = vec![rdf(
            p,
            "u32",
            vec![1, 0, 0, 0],
            Some(vec![0xCC]),
            "u32",
            DfAccessMode::Read,
        )];
        let rt = SetuObjectRuntime::new(vec![], preload, test_tx_hash(), test_address(), 0);

        let hit = (p, "u32".to_string(), vec![1, 0, 0, 0]);
        let miss = (p, "u32".to_string(), vec![9, 9, 9, 9]);
        assert!(rt.df_cache_get(&hit).is_some());
        assert!(rt.df_cache_get(&miss).is_none());
    }

    #[test]
    fn test_input_object_exists() {
        // U3: input_object_exists recognises loaded input IDs only.
        let id_in = ObjectId::new([0x20; 32]);
        let id_out = ObjectId::new([0x21; 32]);
        let rt = SetuObjectRuntime::new(
            vec![make_input_object(id_in, 1)],
            vec![],
            test_tx_hash(),
            test_address(),
            0,
        );
        assert!(rt.input_object_exists(&id_in));
        assert!(!rt.input_object_exists(&id_out));
    }

    #[test]
    fn test_record_df_create_accumulates() {
        // U4: two creates preserved in insertion order.
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let p = ObjectId::new([0x30; 32]);
        let df1 = ObjectId::new([0xD1; 32]);
        let df2 = ObjectId::new([0xD2; 32]);
        let mk = |parent: ObjectId, kb: u8| DfCreateEffect {
            parent,
            key_type_tag: "u64".into(),
            key_bcs: vec![kb, 0, 0, 0, 0, 0, 0, 0],
            value_type_tag: "u64".into(),
            value_bcs: vec![kb; 8],
        };
        rt.record_df_create(df1, mk(p, 1));
        rt.record_df_create(df2, mk(p, 2));
        let r = rt.into_results();
        assert_eq!(r.df_created.len(), 2);
        let ordered: Vec<_> = r.df_created.keys().copied().collect();
        assert_eq!(ordered, vec![df1, df2]);
    }

    #[test]
    fn test_record_df_mutate_overwrites_same_oid() {
        // U5: second mutate on same df_oid wins.
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let df = ObjectId::new([0xDA; 32]);
        let p = ObjectId::new([0x31; 32]);
        let mk = |new_bytes: Vec<u8>| DfMutateEffect {
            parent: p,
            key_type_tag: "u64".into(),
            key_bcs: vec![1; 8],
            value_type_tag: "u64".into(),
            old_value_bcs: vec![0; 8],
            new_value_bcs: new_bytes,
            on_disk_envelope: None,
        };
        rt.record_df_mutate(df, mk(vec![0x01]));
        rt.record_df_mutate(df, mk(vec![0x02]));
        let r = rt.into_results();
        assert_eq!(r.df_mutated.len(), 1);
        assert_eq!(r.df_mutated[&df].new_value_bcs, vec![0x02]);
    }

    #[test]
    fn test_record_df_delete_dedup() {
        // U6: double-delete is idempotent (IndexMap dedup — second insert wins).
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let df = ObjectId::new([0xDB; 32]);
        let p = ObjectId::new([0x32; 32]);
        let mk = || DfDeleteEffect {
            parent: p,
            key_type_tag: "u64".into(),
            key_bcs: vec![2; 8],
            value_type_tag: "u64".into(),
            old_value_bcs: vec![9; 8],
            on_disk_envelope: None,
        };
        rt.record_df_delete(df, mk());
        rt.record_df_delete(df, mk());
        let r = rt.into_results();
        assert_eq!(r.df_deleted.len(), 1);
    }

    /// U-fold-1 (fix-df-mutate-delete-same-oid): `record_df_delete` collapses
    /// any prior `df_mutated` entry into the new Delete and rewrites the
    /// Delete's `old_value_bcs` to the pre-tx value carried by the mutate.
    /// This covers two real same-tx sequences:
    ///   A. `remove → add(v) → remove`  (mutate effect was {old=v0, new=v})
    ///   B. `borrow_mut → remove`       (mutate effect was {old=v0, new=v0})
    /// Both collapse to a single Delete with `old_value_bcs = v0`.
    #[test]
    fn test_record_df_delete_folds_prior_mutate() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let df = ObjectId::new([0xDD; 32]);
        let p = ObjectId::new([0x42; 32]);
        let env_v0 = vec![0xE0; 24];

        // Sequence A: prior mutate has new ≠ old; the post-fold Delete must
        // carry the pre-tx old, NOT the post-mutate new.
        rt.record_df_mutate(
            df,
            DfMutateEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![1; 8],
                value_type_tag: "u64".into(),
                old_value_bcs: vec![0xA; 8], // v0 (pre-tx)
                new_value_bcs: vec![0xB; 8], // v (post-replace)
                on_disk_envelope: Some(env_v0.clone()),
            },
        );
        rt.record_df_delete(
            df,
            DfDeleteEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![1; 8],
                value_type_tag: "u64".into(),
                old_value_bcs: vec![0xB; 8], // post-mutate v (WRONG without fold)
                on_disk_envelope: Some(env_v0.clone()),
            },
        );

        let r = rt.into_results();
        assert!(
            !r.df_mutated.contains_key(&df),
            "fold MUST drop df_mutated[oid]"
        );
        let d = r.df_deleted.get(&df).expect("df_deleted[oid] kept");
        assert_eq!(
            d.old_value_bcs,
            vec![0xA; 8],
            "old_value_bcs MUST be rewritten to pre-tx v0"
        );
        assert_eq!(
            d.on_disk_envelope.as_deref(),
            Some(&env_v0[..]),
            "on_disk_envelope preserved (cache.envelope_bytes invariant)"
        );
    }

    /// U-fold-2: `record_df_delete` is a no-op fold when no prior mutate
    /// exists (i.e. plain remove).
    #[test]
    fn test_record_df_delete_no_prior_mutate_is_passthrough() {
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let df = ObjectId::new([0xDE; 32]);
        let p = ObjectId::new([0x43; 32]);
        rt.record_df_delete(
            df,
            DfDeleteEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![5; 8],
                value_type_tag: "u64".into(),
                old_value_bcs: vec![0xC; 8],
                on_disk_envelope: None,
            },
        );
        let r = rt.into_results();
        assert!(r.df_mutated.is_empty());
        let d = r.df_deleted.get(&df).expect("kept");
        assert_eq!(d.old_value_bcs, vec![0xC; 8], "passthrough — unchanged");
    }

    #[test]
    fn test_df_create_then_delete_both_recorded() {
        // U7: M2 does not cancel create/delete — M3 reconciles.
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let p = ObjectId::new([0x40; 32]);
        let df = ObjectId::new([0xDC; 32]);
        rt.record_df_create(
            df,
            DfCreateEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![7; 8],
                value_type_tag: "u64".into(),
                value_bcs: vec![8; 8],
            },
        );
        rt.record_df_delete(
            df,
            DfDeleteEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![7; 8],
                value_type_tag: "u64".into(),
                old_value_bcs: vec![8; 8],
                on_disk_envelope: None,
            },
        );
        let r = rt.into_results();
        assert!(r.df_created.contains_key(&df));
        assert!(r.df_deleted.contains_key(&df));
    }

    #[test]
    fn test_into_results_exports_df_fields() {
        // U8: all 4 DF fields visible in ObjectRuntimeResults.
        let p = ObjectId::new([0x50; 32]);
        let preload = vec![rdf(
            p,
            "u64",
            vec![1; 8],
            Some(vec![2; 8]),
            "u64",
            DfAccessMode::Mutate,
        )];
        let mut rt = SetuObjectRuntime::new(vec![], preload, test_tx_hash(), test_address(), 0);
        rt.record_df_create(
            ObjectId::new([0xE1; 32]),
            DfCreateEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![3; 8],
                value_type_tag: "u64".into(),
                value_bcs: vec![4; 8],
            },
        );
        rt.record_df_mutate(
            ObjectId::new([0xE2; 32]),
            DfMutateEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![5; 8],
                value_type_tag: "u64".into(),
                old_value_bcs: vec![6; 8],
                new_value_bcs: vec![7; 8],
                on_disk_envelope: None,
            },
        );
        rt.record_df_delete(
            ObjectId::new([0xE3; 32]),
            DfDeleteEffect {
                parent: p,
                key_type_tag: "u64".into(),
                key_bcs: vec![8; 8],
                value_type_tag: "u64".into(),
                old_value_bcs: vec![9; 8],
                on_disk_envelope: None,
            },
        );
        let r = rt.into_results();
        assert_eq!(r.df_created.len(), 1);
        assert_eq!(r.df_mutated.len(), 1);
        assert_eq!(r.df_deleted.len(), 1);
    }

    #[test]
    fn test_df_mutate_effect_roundtrip() {
        // U11: DfMutateEffect fields survive round-trip through runtime.
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let p = ObjectId::new([0x60; 32]);
        let df = ObjectId::new([0xE5; 32]);
        let eff = DfMutateEffect {
            parent: p,
            key_type_tag: "u64".into(),
            key_bcs: vec![10; 8],
            value_type_tag: "u128".into(),
            old_value_bcs: vec![0xAA; 16],
            new_value_bcs: vec![0xBB; 16],
            on_disk_envelope: None,
        };
        rt.record_df_mutate(df, eff);
        let r = rt.into_results();
        let out = &r.df_mutated[&df];
        assert_eq!(out.parent, p);
        assert_eq!(out.key_type_tag, "u64");
        assert_eq!(out.key_bcs, vec![10; 8]);
        assert_eq!(out.value_type_tag, "u128");
        assert_eq!(out.old_value_bcs, vec![0xAA; 16]);
        assert_eq!(out.new_value_bcs, vec![0xBB; 16]);
    }

    #[test]
    fn test_df_delete_effect_roundtrip() {
        // U12: DfDeleteEffect fields survive round-trip through runtime.
        let mut rt = SetuObjectRuntime::new(vec![], vec![], test_tx_hash(), test_address(), 0);
        let p = ObjectId::new([0x61; 32]);
        let df = ObjectId::new([0xE6; 32]);
        let eff = DfDeleteEffect {
            parent: p,
            key_type_tag: "address".into(),
            key_bcs: vec![0xCD; 32],
            value_type_tag: "u64".into(),
            old_value_bcs: vec![0xEF; 8],
            on_disk_envelope: None,
        };
        rt.record_df_delete(df, eff);
        let r = rt.into_results();
        let out = &r.df_deleted[&df];
        assert_eq!(out.parent, p);
        assert_eq!(out.key_type_tag, "address");
        assert_eq!(out.key_bcs, vec![0xCD; 32]);
        assert_eq!(out.value_type_tag, "u64");
        assert_eq!(out.old_value_bcs, vec![0xEF; 8]);
    }
}
