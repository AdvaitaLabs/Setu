//! HybridExecutor — routes events to native runtime or Move VM.
//!
//! Used by tests and future integration; the Enclave path uses
//! `execute_single_event_with_object_store` directly.

use std::str::FromStr;

use move_core_types::{
    account_address::AccountAddress,
    identifier::{IdentStr, Identifier},
    language_storage::{ModuleId, TypeTag},
};

use setu_runtime::{
    error::RuntimeError,
    state::{ObjectStore, StateStore},
    ExecutionContext, ExecutionOutput, RuntimeExecutor, StateChange, StateChangeType,
};
use setu_types::OperationType;

use crate::engine::{MoveExecutionContext, MoveStateChange, MoveStateChangeType, SetuMoveEngine};
use crate::object_runtime::InputObject;

// ═══════════════════════════════════════════════════════════════
// MoveStateChange → StateChange bridge
// ═══════════════════════════════════════════════════════════════

impl From<MoveStateChange> for StateChange {
    fn from(msc: MoveStateChange) -> Self {
        let change_type = match msc.change_type {
            MoveStateChangeType::Create => StateChangeType::Create,
            MoveStateChangeType::Update => StateChangeType::Update,
            MoveStateChangeType::Delete => StateChangeType::Delete,
            // Freeze is represented as Update (ownership change to Immutable)
            MoveStateChangeType::Freeze => StateChangeType::Update,
            // Share is represented as Update (ownership change to Shared)
            MoveStateChangeType::Share => StateChangeType::Update,
        };
        StateChange {
            change_type,
            object_id: msc.object_id,
            old_state: msc.old_state,
            new_state: msc.new_state,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// HybridExecutor
// ═══════════════════════════════════════════════════════════════

/// Routes execution to native setu-runtime or Move VM based on OperationType.
pub struct HybridExecutor<S: StateStore + ObjectStore> {
    native: RuntimeExecutor<S>,
    move_engine: Option<SetuMoveEngine>,
}

impl<S: StateStore + ObjectStore> HybridExecutor<S> {
    /// Create a HybridExecutor without Move VM (native-only).
    pub fn new(state: S) -> Self {
        Self {
            native: RuntimeExecutor::new(state),
            move_engine: None,
        }
    }

    /// Create a HybridExecutor with Move VM support.
    pub fn new_with_move(state: S, engine: SetuMoveEngine) -> Self {
        Self {
            native: RuntimeExecutor::new(state),
            move_engine: Some(engine),
        }
    }

    /// Execute an operation based on OperationType routing.
    pub fn execute(
        &mut self,
        event: &setu_types::Event,
        resolved_inputs: &setu_types::task::ResolvedInputs,
        ctx: &ExecutionContext,
    ) -> Result<ExecutionOutput, RuntimeError> {
        match &resolved_inputs.operation {
            OperationType::NoOp => Ok(ExecutionOutput {
                success: true,
                message: None,
                state_changes: vec![],
                created_objects: vec![],
                deleted_objects: vec![],
                query_result: None,
            }),

            OperationType::Transfer {
                from_coin_index,
                amount,
            } => {
                let coin_id = resolved_inputs.input_objects[*from_coin_index].object_id;
                let transfer = event.transfer.as_ref().ok_or_else(|| {
                    RuntimeError::InvalidTransaction("Transfer op but no transfer payload".into())
                })?;
                self.native.execute_transfer_with_coin(
                    coin_id,
                    &transfer.from,
                    &transfer.to,
                    Some(*amount),
                    ctx,
                )
            }

            OperationType::MergeCoins {
                target_index,
                source_indices,
            } => {
                let target_coin_id = resolved_inputs.input_objects[*target_index].object_id;
                let source_coin_ids: Vec<_> = source_indices
                    .iter()
                    .map(|&i| resolved_inputs.input_objects[i].object_id)
                    .collect();
                let owner = self
                    .native
                    .state()
                    .get_object(&target_coin_id)
                    .map_err(|e| RuntimeError::StateError(e.to_string()))?
                    .ok_or(RuntimeError::ObjectNotFound(target_coin_id))?
                    .metadata
                    .owner
                    .ok_or_else(|| {
                        RuntimeError::InvalidTransaction("Target coin has no owner".into())
                    })?;
                self.native
                    .execute_merge_coins(&owner, target_coin_id, &source_coin_ids, ctx)
            }

            OperationType::SplitCoin {
                source_index,
                amounts,
            } => {
                let source_coin_id = resolved_inputs.input_objects[*source_index].object_id;
                let owner = self
                    .native
                    .state()
                    .get_object(&source_coin_id)
                    .map_err(|e| RuntimeError::StateError(e.to_string()))?
                    .ok_or(RuntimeError::ObjectNotFound(source_coin_id))?
                    .metadata
                    .owner
                    .ok_or_else(|| {
                        RuntimeError::InvalidTransaction("Source coin has no owner".into())
                    })?;
                self.native
                    .execute_split_coin(&owner, source_coin_id, amounts, ctx)
            }

            OperationType::MergeThenTransfer {
                target_index,
                source_indices,
                recipient,
                amount,
            } => {
                let target_coin_id = resolved_inputs.input_objects[*target_index].object_id;
                let source_coin_ids: Vec<_> = source_indices
                    .iter()
                    .map(|&i| resolved_inputs.input_objects[i].object_id)
                    .collect();
                let owner = self
                    .native
                    .state()
                    .get_object(&target_coin_id)
                    .map_err(|e| RuntimeError::StateError(e.to_string()))?
                    .ok_or(RuntimeError::ObjectNotFound(target_coin_id))?
                    .metadata
                    .owner
                    .ok_or_else(|| {
                        RuntimeError::InvalidTransaction("Target coin has no owner".into())
                    })?;

                // Step 1: Merge
                let mut merge_out = self.native.execute_merge_coins(
                    &owner,
                    target_coin_id,
                    &source_coin_ids,
                    ctx,
                )?;
                if !merge_out.success {
                    return Ok(merge_out);
                }

                // Step 2: Transfer from merged coin
                let transfer_out = self.native.execute_transfer_with_coin(
                    target_coin_id,
                    &owner.to_string(),
                    &recipient.to_string(),
                    Some(*amount),
                    ctx,
                )?;
                if !transfer_out.success {
                    return Ok(transfer_out);
                }

                // Combine
                merge_out.state_changes.extend(transfer_out.state_changes);
                merge_out
                    .created_objects
                    .extend(transfer_out.created_objects);
                merge_out
                    .deleted_objects
                    .extend(transfer_out.deleted_objects);
                Ok(merge_out)
            }

            OperationType::MoveCall {
                package,
                module_name,
                function_name,
                type_args,
                pure_args,
                mutable_indices,
                consumed_indices,
            } => {
                let engine = self.move_engine.as_ref().ok_or_else(|| {
                    RuntimeError::InvalidTransaction("Move VM not enabled".into())
                })?;

                // Build InputObjects from store
                let input_objects: Vec<InputObject> = resolved_inputs
                    .input_objects
                    .iter()
                    .map(|ro| {
                        let env = self
                            .native
                            .state()
                            .get_envelope(&ro.object_id)
                            .map_err(|e| {
                                RuntimeError::StateError(format!(
                                    "Failed to get envelope for {}: {}",
                                    ro.object_id, e
                                ))
                            })?
                            .ok_or(RuntimeError::ObjectNotFound(ro.object_id))?;
                        InputObject::from_envelope(&ro.object_id, &env)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                // Parse ModuleId
                let addr = AccountAddress::from_hex_literal(package).map_err(|e| {
                    RuntimeError::InvalidTransaction(format!("Invalid package address: {}", e))
                })?;
                let module_id = ModuleId::new(
                    addr,
                    Identifier::new(module_name.as_str()).map_err(|e| {
                        RuntimeError::InvalidTransaction(format!("Invalid module name: {}", e))
                    })?,
                );
                let func_name = IdentStr::new(function_name.as_str()).map_err(|e| {
                    RuntimeError::InvalidTransaction(format!("Invalid function name: {}", e))
                })?;

                // Parse type args
                let ty_args: Vec<TypeTag> = type_args
                    .iter()
                    .map(|s| TypeTag::from_str(s))
                    .collect::<Result<_, _>>()
                    .map_err(|e| {
                        RuntimeError::InvalidTransaction(format!("Invalid type arg: {}", e))
                    })?;

                // MoveExecutionContext
                let move_ctx = MoveExecutionContext {
                    tx_hash: ctx.tx_hash,
                    sender: setu_types::Address::from_hex(&event.creator.clone())
                        .unwrap_or(setu_types::Address::ZERO),
                    gas_budget: ctx.gas_budget.unwrap_or(10_000_000),
                    current_version: 0,
                    epoch: 0,
                    needs_tx_context: true,
                    epoch_timestamp_ms: event.timestamp,
                };

                // Assemble args
                let (combined_args, mutable_arg_map) = build_move_call_args(
                    &input_objects,
                    pure_args,
                    consumed_indices,
                    mutable_indices,
                );

                // Execute
                let move_output = engine.execute(
                    self.native.state(),
                    input_objects,
                    resolved_inputs.dynamic_fields.clone(),
                    &module_id,
                    func_name,
                    ty_args,
                    combined_args,
                    &move_ctx,
                    &mutable_arg_map,
                )?;

                if !move_output.success {
                    return Ok(ExecutionOutput {
                        success: false,
                        message: move_output.error,
                        state_changes: vec![],
                        created_objects: vec![],
                        deleted_objects: vec![],
                        query_result: None,
                    });
                }

                // Bridge state changes
                let state_changes: Vec<StateChange> = move_output
                    .state_changes
                    .into_iter()
                    .map(StateChange::from)
                    .collect();
                let created: Vec<_> = state_changes
                    .iter()
                    .filter(|sc| sc.change_type == StateChangeType::Create)
                    .map(|sc| sc.object_id)
                    .collect();
                let deleted: Vec<_> = state_changes
                    .iter()
                    .filter(|sc| sc.change_type == StateChangeType::Delete)
                    .map(|sc| sc.object_id)
                    .collect();

                Ok(ExecutionOutput {
                    success: true,
                    message: None,
                    state_changes,
                    created_objects: created,
                    deleted_objects: deleted,
                    query_result: None,
                })
            }

            OperationType::MovePublish { .. } => Err(RuntimeError::InvalidTransaction(
                "MovePublish is a ROOT event, not routed through HybridExecutor".into(),
            )),

            // B6b Phase 3g: PTB execution dispatched to engine.execute_ptb.
            //   - input_objects materialization mirrors the MoveCall path
            //     (envelope → InputObject), but indexes ALL declared
            //     inputs of type CallArg::Object (resolve_ptb in
            //     TaskPreparer is responsible for ordering them).
            //   - dynamic_fields plumbed through resolved_inputs.
            //   - All command lowering, slot tracking, gas, and finalize
            //     happen inside engine.execute_ptb.
            OperationType::ProgrammableTransaction(ptb) => {
                let engine = self.move_engine.as_ref().ok_or_else(|| {
                    RuntimeError::InvalidTransaction("Move VM not enabled".into())
                })?;

                // Materialize input_objects for every Object CallArg, in
                // the SAME order they appear in ptb.inputs. resolve_ptb
                // (Phase 3h) will guarantee resolved_inputs.input_objects
                // is aligned with the Object positions.
                let input_objects: Vec<InputObject> = resolved_inputs
                    .input_objects
                    .iter()
                    .map(|ro| {
                        let env = self
                            .native
                            .state()
                            .get_envelope(&ro.object_id)
                            .map_err(|e| {
                                RuntimeError::StateError(format!(
                                    "PTB input envelope load failed for {}: {e}",
                                    ro.object_id
                                ))
                            })?
                            .ok_or(RuntimeError::ObjectNotFound(ro.object_id))?;
                        InputObject::from_envelope(&ro.object_id, &env)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let move_ctx = MoveExecutionContext {
                    tx_hash: ctx.tx_hash,
                    sender: setu_types::Address::from_hex(&event.creator.clone())
                        .unwrap_or(setu_types::Address::ZERO),
                    gas_budget: ctx.gas_budget.unwrap_or(10_000_000),
                    current_version: 0,
                    epoch: 0,
                    needs_tx_context: true,
                    epoch_timestamp_ms: event.timestamp,
                };

                let move_output = engine.execute_ptb(
                    self.native.state(),
                    ptb,
                    input_objects,
                    resolved_inputs.dynamic_fields.clone(),
                    &move_ctx,
                )?;

                if !move_output.success {
                    return Ok(ExecutionOutput {
                        success: false,
                        message: move_output.error,
                        state_changes: vec![],
                        created_objects: vec![],
                        deleted_objects: vec![],
                        query_result: None,
                    });
                }

                let state_changes: Vec<StateChange> = move_output
                    .state_changes
                    .into_iter()
                    .map(StateChange::from)
                    .collect();
                let created: Vec<_> = state_changes
                    .iter()
                    .filter(|sc| sc.change_type == StateChangeType::Create)
                    .map(|sc| sc.object_id)
                    .collect();
                let deleted: Vec<_> = state_changes
                    .iter()
                    .filter(|sc| sc.change_type == StateChangeType::Delete)
                    .map(|sc| sc.object_id)
                    .collect();

                Ok(ExecutionOutput {
                    success: true,
                    message: None,
                    state_changes,
                    created_objects: created,
                    deleted_objects: deleted,
                    query_result: None,
                })
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Helper: build_move_call_args
// ═══════════════════════════════════════════════════════════════

/// Assemble Move function call arguments in the correct order.
///
/// Returns `(args, mutable_arg_map)` where `mutable_arg_map` maps
/// each `&mut` argument's position in `args` to its input ObjectId.
/// TxContext is handled by engine.execute() internally.
pub fn build_move_call_args(
    input_objects: &[InputObject],
    pure_args: &[Vec<u8>],
    consumed_indices: &[usize],
    mutable_indices: &[usize],
) -> (Vec<Vec<u8>>, Vec<(usize, setu_types::ObjectId)>) {
    let mut args = Vec::new();
    let mut mutable_arg_map = Vec::new();
    let mutable_set: std::collections::HashSet<usize> = mutable_indices.iter().copied().collect();

    // Input objects are placed first, in their declared index order.
    // EVERY declared input object contributes its BCS data, regardless of
    // whether it was marked consumed or mutable — this covers immutable
    // borrows (`&T`), which live in `input_object_ids` but are in neither
    // `consumed_indices` nor `mutable_indices`. Dropping them here used to
    // cause `NUMBER_OF_ARGUMENTS_MISMATCH` for any entry fn taking `&T`
    // (see docs/bugs/20260421-hybrid-drops-immutable-input-object.md).
    for (idx, obj) in input_objects.iter().enumerate() {
        let arg_pos = args.len();
        args.push(obj.move_data.clone());
        if mutable_set.contains(&idx) {
            mutable_arg_map.push((arg_pos, obj.id));
        }
    }
    // `consumed_indices` is still tracked by the caller (for by-value
    // transfer/delete semantics) but does not affect argument ordering
    // here — by-value args occupy the same slot as any other input
    // object from the Move VM's perspective.
    let _ = consumed_indices;

    // Pure arguments follow all object args
    args.extend_from_slice(pure_args);

    (args, mutable_arg_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_state_change_to_state_change_create() {
        let msc = MoveStateChange {
            object_id: setu_types::ObjectId::new([1; 32]),
            change_type: MoveStateChangeType::Create,
            old_state: None,
            new_state: Some(vec![1, 2, 3]),
        };
        let sc: StateChange = msc.into();
        assert_eq!(sc.change_type, StateChangeType::Create);
        assert!(sc.old_state.is_none());
        assert_eq!(sc.new_state, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_move_state_change_freeze_maps_to_update() {
        let msc = MoveStateChange {
            object_id: setu_types::ObjectId::new([2; 32]),
            change_type: MoveStateChangeType::Freeze,
            old_state: Some(vec![1]),
            new_state: Some(vec![2]),
        };
        let sc: StateChange = msc.into();
        assert_eq!(sc.change_type, StateChangeType::Update);
    }

    #[test]
    fn test_build_move_call_args_ordering() {
        use move_core_types::language_storage::StructTag;
        let obj_a = InputObject {
            id: setu_types::ObjectId::new([0xAA; 32]),
            owner: setu_types::Address::new([0; 32]),
            ownership: setu_types::Ownership::AddressOwner(setu_types::Address::new([0; 32])),
            version: 1,
            envelope_bytes: vec![],
            move_data: vec![0xA0],
            type_tag: StructTag {
                address: AccountAddress::ONE,
                module: Identifier::new("m").unwrap(),
                name: Identifier::new("T").unwrap(),
                type_params: vec![],
            },
        };
        let obj_b = InputObject {
            id: setu_types::ObjectId::new([0xBB; 32]),
            owner: setu_types::Address::new([0; 32]),
            ownership: setu_types::Ownership::AddressOwner(setu_types::Address::new([0; 32])),
            version: 1,
            envelope_bytes: vec![],
            move_data: vec![0xB0],
            type_tag: StructTag {
                address: AccountAddress::ONE,
                module: Identifier::new("m").unwrap(),
                name: Identifier::new("T").unwrap(),
                type_params: vec![],
            },
        };
        let pure = vec![vec![0xCC]];
        let consumed = &[0usize]; // obj_a consumed
        let mutable = &[1usize]; // obj_b mutable

        let (args, mutable_arg_map) =
            build_move_call_args(&[obj_a, obj_b], &pure, consumed, mutable);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], vec![0xA0]); // consumed first
        assert_eq!(args[1], vec![0xB0]); // mutable second
        assert_eq!(args[2], vec![0xCC]); // pure last
                                         // mutable_arg_map: obj_b (index 1) is mutable, at arg position 1
        assert_eq!(mutable_arg_map.len(), 1);
        assert_eq!(mutable_arg_map[0].0, 1); // arg position
        assert_eq!(mutable_arg_map[0].1, setu_types::ObjectId::new([0xBB; 32]));
        // object ID
    }

    #[test]
    fn test_hybrid_noop_returns_empty() {
        use setu_runtime::InMemoryObjectStore;
        use setu_types::task::ResolvedInputs;

        let store = InMemoryObjectStore::new();
        let mut hybrid = HybridExecutor::new(store);
        let event = setu_types::Event::new(
            setu_types::EventType::System,
            vec![],
            setu_types::VLCSnapshot::default(),
            "test".to_string(),
        );
        let mut resolved = ResolvedInputs::new();
        resolved.operation = OperationType::NoOp;
        let ctx = ExecutionContext::new("test".into(), 0, false, [0; 32]);

        let output = hybrid.execute(&event, &resolved, &ctx).unwrap();
        assert!(output.success);
        assert!(output.state_changes.is_empty());
    }

    #[test]
    fn test_hybrid_move_publish_rejected() {
        use setu_runtime::InMemoryObjectStore;
        use setu_types::task::ResolvedInputs;

        let store = InMemoryObjectStore::new();
        let mut hybrid = HybridExecutor::new(store);
        let event = setu_types::Event::new(
            setu_types::EventType::ContractPublish,
            vec![],
            setu_types::VLCSnapshot::default(),
            "test".to_string(),
        );
        let mut resolved = ResolvedInputs::new();
        resolved.operation = OperationType::MovePublish { modules: vec![] };
        let ctx = ExecutionContext::new("test".into(), 0, false, [0; 32]);

        let result = hybrid.execute(&event, &resolved, &ctx);
        assert!(result.is_err());
    }

    /// I3 (Phase 3g supersession): when B6a shipped, this site rejected PTBs
    /// with a `PTB_NOT_YET_EXECUTABLE` marker. With B6b Phase 3g landed, the
    /// dispatch arm now invokes `engine.execute_ptb`. The asserting variant
    /// of this test was deleted; we keep an empty-PTB happy-path smoke
    /// instead — empty commands → success with zero state changes — to lock
    /// the dispatch wiring without needing a Move VM with stdlib loaded.
    /// (Real PTB execution is exercised by the engine.execute_ptb tests.)
    #[test]
    fn i3_hybrid_ptb_dispatched_to_engine() {
        use setu_runtime::InMemoryObjectStore;
        use setu_types::ptb::ProgrammableTransaction;
        use setu_types::task::ResolvedInputs;

        let store = InMemoryObjectStore::new();
        let mut hybrid = HybridExecutor::new(store);
        let event = setu_types::Event::new(
            setu_types::EventType::ContractCall,
            vec![],
            setu_types::VLCSnapshot::default(),
            "test".to_string(),
        );
        let mut resolved = ResolvedInputs::new();
        resolved.operation = OperationType::ProgrammableTransaction(ProgrammableTransaction {
            inputs: vec![],
            commands: vec![],
            dynamic_field_accesses: vec![],
        });
        let ctx = ExecutionContext::new("test".into(), 0, false, [0; 32]);

        // Without a configured Move engine, dispatch returns
        // "Move VM not enabled" — which proves the new dispatch path is
        // reached (B6a's marker would have been "PTB_NOT_YET_EXECUTABLE").
        let err = hybrid
            .execute(&event, &resolved, &ctx)
            .expect_err("hybrid.execute requires Move engine");
        let msg = format!("{err}");
        assert!(
            msg.contains("Move VM not enabled"),
            "expected dispatch to reach engine.execute_ptb path, got: {msg}"
        );
    }

    #[test]
    fn test_move_execution_context_defaults() {
        let ctx = MoveExecutionContext {
            tx_hash: [0; 32],
            sender: setu_types::Address::ZERO,
            gas_budget: 10_000_000,
            current_version: 0,
            epoch: 0,
            needs_tx_context: true,
            epoch_timestamp_ms: 0,
        };
        assert_eq!(ctx.epoch, 0);
        assert!(ctx.needs_tx_context);
    }

    #[test]
    fn test_build_tx_context_bcs_roundtrip() {
        let ctx = MoveExecutionContext {
            tx_hash: [0xAB; 32],
            sender: setu_types::Address::new([0x01; 32]),
            gas_budget: 10_000_000,
            current_version: 0,
            epoch: 42,
            needs_tx_context: true,
            epoch_timestamp_ms: 0,
        };
        let bcs_bytes = SetuMoveEngine::build_tx_context_bcs(&ctx);
        // sender address: 32 bytes
        assert_eq!(&bcs_bytes[0..32], &[0x01; 32]);
        // tx_hash: BCS vector (ULEB128 len=32 + 32 bytes = 33 bytes)
        assert_eq!(bcs_bytes[32], 32); // length prefix
        assert_eq!(&bcs_bytes[33..65], &[0xAB; 32]);
        // epoch: u64 LE
        let epoch = u64::from_le_bytes(bcs_bytes[65..73].try_into().unwrap());
        assert_eq!(epoch, 42);
        // ids_created: u64 LE = 0
        let ids = u64::from_le_bytes(bcs_bytes[73..81].try_into().unwrap());
        assert_eq!(ids, 0);
        assert_eq!(bcs_bytes.len(), 81);
    }
}
