//! Move VM request handlers (Phase 4)
//!
//! Handles MoveCall and MovePublish HTTP submissions.
//! Follows the TransferHandler unit-struct pattern — all deps as function params.

use crate::InfraExecutor;
use crate::RouterManager;
use crate::TaskPreparer;
use super::tee_executor::TeeExecutor;
use setu_api::{MoveCallRequest, MoveCallResponse, MovePublishRequest, MovePublishResponse};
use setu_types::event::{Event, MoveCallPayload, MovePtbPayload, VLCSnapshot};
use setu_types::object::ObjectId;
use setu_types::ptb::ProgrammableTransaction;
use setu_types::SubnetId;
use std::sync::Arc;
use tracing::{error, info, warn};

/// MoveCall handler — unit struct matching TransferHandler pattern
pub struct MoveCallHandler;

impl MoveCallHandler {
    /// Process a MoveCall submission
    ///
    /// Flow: convert request → Event → TaskPreparer.prepare_move_call_task()
    ///       → route to solver → TeeExecutor.execute_solver_inline_batch()
    ///       → spawn consensus → return result
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_move_call(
        validator_id: &str,
        task_preparer: &TaskPreparer,
        router_manager: &RouterManager,
        tee_executor: &TeeExecutor,
        state_provider: &Arc<setu_storage::MerkleStateProvider>,
        vlc_time: u64,
        request: MoveCallRequest,
    ) -> MoveCallResponse {
        // 1. Convert MoveCallRequest → MoveCallPayload
        let mut payload = match Self::convert_request(&request) {
            Ok(p) => p,
            Err(e) => {
                return MoveCallResponse {
                    event_id: String::new(),
                    success: false,
                    state_changes: 0,
                    created_objects: vec![],
                    error: Some(e),
                };
            }
        };

        // 1.5. Auto-detect needs_tx_context from module bytecode
        //      Look up the target module from storage or embedded stdlib.
        {
            let module_key = format!("mod:{}::{}", payload.package, payload.module);
            let module_bytes = state_provider.get_raw_data(&module_key)
                .or_else(|| {
                    // Check embedded stdlib if target is at address 0x1
                    let stripped = payload.package.strip_prefix("0x").unwrap_or(&payload.package);
                    if stripped == "1" || stripped == "0000000000000000000000000000000000000000000000000000000000000001" {
                        setu_move_vm::engine::STDLIB_MODULES.iter()
                            .find(|(name, _)| *name == payload.module.as_str())
                            .map(|(_, bytes)| bytes.to_vec())
                    } else {
                        None
                    }
                });
            if let Some(bytes) = module_bytes {
                if let Some(detected) = setu_move_vm::engine::detect_needs_tx_context(&bytes, &payload.function) {
                    if detected != payload.needs_tx_context {
                        info!(
                            function = %payload.function,
                            declared = payload.needs_tx_context,
                            detected,
                            "Auto-detected needs_tx_context override"
                        );
                        payload.needs_tx_context = detected;
                    }
                }
            }
        }

        // 2. Build VLCSnapshot
        let vlc_snapshot = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: vlc_time,
            physical_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };

        // 3. Create ContractCall event
        let event = Event::move_call(
            payload.clone(),
            vec![],
            vlc_snapshot,
            validator_id.to_string(),
        );

        // 4. Determine subnet
        let subnet_id = match &request.subnet_id {
            Some(s) if s != "ROOT" => {
                warn!(subnet = %s, "Custom subnet not supported for MoveCall, using ROOT");
                SubnetId::ROOT
            }
            _ => SubnetId::ROOT,
        };

        // 5. Prepare SolverTask via TaskPreparer
        let solver_task = match task_preparer.prepare_move_call_task(&event, &payload, subnet_id) {
            Ok(task) => task,
            Err(e) => {
                error!(error = %e, "MoveCall task preparation failed");
                return MoveCallResponse {
                    event_id: String::new(),
                    success: false,
                    state_changes: 0,
                    created_objects: vec![],
                    error: Some(format!("Task preparation failed: {}", e)),
                };
            }
        };

        // 6. Route to solver
        let solver_id = match router_manager.route_any() {
            Ok(id) => id,
            Err(e) => {
                error!(error = %e, "No solver available for MoveCall");
                return MoveCallResponse {
                    event_id: String::new(),
                    success: false,
                    state_changes: 0,
                    created_objects: vec![],
                    error: Some(format!("No solver available: {}", e)),
                };
            }
        };

        // 7. Execute via TeeExecutor (no coin reservations needed for MoveCall)
        let call_id = format!("move-call-{}", vlc_time);
        match tee_executor.execute_solver_inline_batch(
            &call_id, &solver_id, solver_task, vec![],
        ).await {
            Ok((result_event, execution_time_us, events_processed)) => {
                let event_id = result_event.id.clone();
                let exec_result = result_event.execution_result.as_ref();
                let state_changes = exec_result
                    .map(|r| r.state_changes.len())
                    .unwrap_or(0);
                let success = exec_result
                    .map(|r| r.success)
                    .unwrap_or(false);
                let error = if success {
                    None
                } else {
                    exec_result.and_then(|r| r.message.clone())
                };

                // Debug: log all state change keys
                if let Some(r) = exec_result {
                    for sc in &r.state_changes {
                        info!(
                            key = %sc.key,
                            has_old = sc.old_value.is_some(),
                            has_new = sc.new_value.is_some(),
                            "MoveCall state_change entry"
                        );
                    }
                }

                // Extract created object keys from state changes
                // Created objects have new_value=Some but old_value=None, key starts with "oid:"
                let created_objects: Vec<String> = exec_result
                    .map(|r| {
                        r.state_changes.iter()
                            .filter(|sc| sc.key.starts_with("oid:") && sc.new_value.is_some() && sc.old_value.is_none())
                            .map(|sc| sc.key.clone())
                            .collect()
                    })
                    .unwrap_or_default();

                // Stage MoveCall state_changes into the speculative overlay so
                // the same client can immediately read-your-writes from this
                // validator. Pre-apply MUST NOT touch the write GSM directly:
                // doing so diverges the SMT across validators after leader
                // rotation (see docs/feat/follower-apply-root-mismatch/design.md,
                // OBS-023, docs/bugs/20260422-follower-apply-root-mismatch.md).
                //
                // Overlay entries are cleared by anchor_builder.rs on CF finalize
                // (both commit_build leader path and apply_follower_finalized_cf
                // follower path); the canonical SMT is written by
                // apply_committed_events at that same point.
                if success {
                    if let Some(r) = result_event.execution_result.as_ref() {
                        let shared = state_provider.shared_state_manager();
                        match shared.stage_overlay(
                            &result_event.id,
                            SubnetId::ROOT,
                            &r.state_changes,
                        ) {
                            Ok(()) => {
                                tracing::debug!(
                                    event_id = %result_event.id,
                                    change_count = r.state_changes.len(),
                                    "MoveCall result staged to speculative overlay"
                                );
                            }
                            Err(e) => {
                                // G11 violation coming out of TEE. Do NOT fall
                                // back to apply_state_change — that would
                                // reintroduce the cross-validator divergence
                                // this fix targets. CF finalize will still
                                // apply the canonical state_changes via
                                // apply_committed_events on every validator.
                                error!(
                                    event_id = %result_event.id,
                                    error = %e,
                                    "MoveCall state_change has malformed key; overlay stage skipped"
                                );
                            }
                        }
                    }
                }

                // Spawn consensus submission
                tee_executor.spawn_post_execution(
                    call_id, result_event, execution_time_us, events_processed,
                );

                info!(
                    event_id = %event_id,
                    state_changes,
                    created_objects = ?created_objects,
                    solver_id = %solver_id,
                    "MoveCall executed"
                );

                MoveCallResponse {
                    event_id,
                    success,
                    state_changes,
                    created_objects,
                    error,
                }
            }
            Err(e) => {
                error!(error = %e, "MoveCall TEE execution failed");
                MoveCallResponse {
                    event_id: String::new(),
                    success: false,
                    state_changes: 0,
                    created_objects: vec![],
                    error: Some(format!("Execution failed: {}", e)),
                }
            }
        }
    }

    /// Convert HTTP request to internal MoveCallPayload
    fn convert_request(request: &MoveCallRequest) -> Result<MoveCallPayload, String> {
        // Resolve sender to canonical hex address (handles both "alice" and "0x..." formats)
        let sender_hex = Self::resolve_address(&request.sender);

        // Decode hex args to raw bytes
        let args: Vec<Vec<u8>> = request.args.iter()
            .map(|hex_str| {
                hex::decode(hex_str.strip_prefix("0x").unwrap_or(hex_str))
                    .map_err(|e| format!("Invalid hex in arg: {}", e))
            })
            .collect::<Result<_, _>>()?;

        // Decode hex object IDs (owned)
        let input_object_ids: Vec<ObjectId> = request.input_object_ids.iter()
            .map(|hex_str| {
                ObjectId::from_hex(hex_str)
                    .map_err(|e| format!("Invalid ObjectId '{}': {}", hex_str, e))
            })
            .collect::<Result<_, _>>()?;

        // Decode hex object IDs (shared, PWOO)
        let shared_object_ids: Vec<ObjectId> = request.shared_object_ids.iter()
            .map(|hex_str| {
                ObjectId::from_hex(hex_str)
                    .map_err(|e| format!("Invalid shared ObjectId '{}': {}", hex_str, e))
            })
            .collect::<Result<_, _>>()?;

        Ok(MoveCallPayload {
            sender: sender_hex,
            package: request.package.clone(),
            module: request.module.clone(),
            function: request.function.clone(),
            type_args: request.type_args.clone(),
            args,
            input_object_ids,
            shared_object_ids,
            mutable_indices: if request.mutable_indices.is_empty() { None } else { Some(request.mutable_indices.clone()) },
            consumed_indices: if request.consumed_indices.is_empty() { None } else { Some(request.consumed_indices.clone()) },
            needs_tx_context: request.needs_tx_context,
            // DF FDP M4: network path does not surface DF declarations yet.
            // Clients speaking the RPC layer get empty DF accesses; HTTP /
            // JSON clients can still populate via serde default.
            dynamic_field_accesses: Vec::new(),
        })
    }

    /// Resolve a human-readable name or hex string to a canonical hex address.
    /// Names like "alice" are hashed via blake3 to produce a deterministic address.
    fn resolve_address(name: &str) -> String {
        let stripped = name.strip_prefix("0x").unwrap_or(name);
        if stripped.len() == 64 && stripped.chars().all(|c| c.is_ascii_hexdigit()) {
            return format!("0x{}", stripped);
        }
        let hash = blake3::hash(name.as_bytes());
        format!("0x{}", hex::encode(hash.as_bytes()))
    }
}

/// MovePublish handler — unit struct matching TransferHandler pattern
pub struct MovePublishHandler;

impl MovePublishHandler {
    /// Process a ContractPublish submission
    ///
    /// Flow: decode hex modules → InfraExecutor.execute_contract_publish() → return (response, event)
    pub async fn submit_move_publish(
        infra_executor: &InfraExecutor,
        vlc_time: u64,
        request: MovePublishRequest,
    ) -> (MovePublishResponse, Option<Event>) {
        // 1. Validate & decode modules from hex
        if request.modules.is_empty() {
            return (MovePublishResponse {
                event_id: String::new(),
                module_count: 0,
                success: false,
                error: Some("Empty module list".into()),
            }, None);
        }

        let modules_bytes: Vec<Vec<u8>> = match request.modules.iter()
            .map(|hex_str| hex::decode(hex_str.strip_prefix("0x").unwrap_or(hex_str))
                .map_err(|e| format!("Invalid hex in module: {}", e)))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(m) => m,
            Err(e) => {
                return (MovePublishResponse {
                    event_id: String::new(),
                    module_count: 0,
                    success: false,
                    error: Some(e),
                }, None);
            }
        };

        // 2. Build VLCSnapshot
        let vlc_snapshot = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: vlc_time,
            physical_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };

        // 3. Execute via InfraExecutor
        let module_count = modules_bytes.len();
        match infra_executor.execute_contract_publish(&request.sender, &modules_bytes, vlc_snapshot) {
            Ok(event) => {
                let event_id = event.id.clone();
                info!(event_id = %event_id, module_count, "MovePublish executed successfully");
                (MovePublishResponse {
                    event_id,
                    module_count,
                    success: true,
                    error: None,
                }, Some(event))
            }
            Err(e) => {
                warn!(error = %e, "MovePublish execution failed");
                (MovePublishResponse {
                    event_id: String::new(),
                    module_count: 0,
                    success: false,
                    error: Some(e),
                }, None)
            }
        }
    }
}

/// PTB handler — unit struct matching MoveCallHandler pattern.
///
/// Wires the HTTP entry `/api/v1/move/ptb` end-to-end:
///   request → MovePtbPayload → Event::move_ptb (EventType::ContractCall)
///         → TaskPreparer.prepare_move_ptb_task
///         → RouterManager.route_any
///         → TeeExecutor.execute_solver_inline_batch
///         → stage_overlay (RYW)
///         → spawn_post_execution (consensus)
///
/// EventType reuse (not a new variant): see
/// `docs/feat/move-vm-phase9-ptb-event-wire/design.md` §4.
pub struct MovePtbHandler;

impl MovePtbHandler {
    /// Process a PTB submission. The caller (service.rs) is responsible for
    /// hex-decoding the BCS-wrapped PTB and running `validate_wire()` first;
    /// this method receives a fully-deserialised `ProgrammableTransaction`.
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_move_ptb(
        validator_id: &str,
        task_preparer: &TaskPreparer,
        router_manager: &RouterManager,
        tee_executor: &TeeExecutor,
        state_provider: &Arc<setu_storage::MerkleStateProvider>,
        vlc_time: u64,
        sender: String,
        ptb: ProgrammableTransaction,
        subnet_id_hint: Option<String>,
        gas_budget: Option<u64>,
    ) -> setu_api::MovePtbResponse {
        // B6c · resolve + validate gas_budget BEFORE any task work. Default
        // sits at `MAX_GAS_BUDGET / 5` so a typical PTB has plenty of head
        // room without blanket-permitting the absolute ceiling.
        let resolved_gas: u64 = match gas_budget {
            Some(b) => b,
            None => setu_move_vm::gas::MAX_GAS_BUDGET / 5,
        };
        if resolved_gas < setu_move_vm::gas::MIN_GAS_PTB
            || resolved_gas > setu_move_vm::gas::MAX_GAS_BUDGET
        {
            return setu_api::MovePtbResponse {
                event_id: String::new(),
                success: false,
                error: Some(format!(
                    "gas_budget {} outside [{}..{}]",
                    resolved_gas,
                    setu_move_vm::gas::MIN_GAS_PTB,
                    setu_move_vm::gas::MAX_GAS_BUDGET,
                )),
                code: None,
            };
        }

        // 1. Resolve sender to canonical hex address.
        let sender_hex = MoveCallHandler::resolve_address(&sender);
        let payload = MovePtbPayload { sender: sender_hex, ptb };

        // 2. Build VLCSnapshot.
        let vlc_snapshot = VLCSnapshot {
            vector_clock: setu_vlc::VectorClock::new(),
            logical_time: vlc_time,
            physical_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };

        // 3. Create the Event (ContractCall + MovePtb payload).
        let event = Event::move_ptb(
            payload.clone(),
            vec![],
            vlc_snapshot,
            validator_id.to_string(),
        );

        // 4. Subnet routing — D6: PTB only runs on ROOT in Phase 1.
        let subnet_id = match subnet_id_hint.as_deref() {
            Some(s) if s != "ROOT" => {
                warn!(subnet = %s, "Custom subnet not supported for PTB, using ROOT");
                SubnetId::ROOT
            }
            _ => SubnetId::ROOT,
        };

        // 5. Prepare SolverTask.
        let solver_task = match task_preparer.prepare_move_ptb_task(&event, &payload, subnet_id) {
            Ok(mut t) => {
                // B6c · stamp the validated PTB gas budget onto the task so
                // it propagates through the solver / TEE path into
                // `MoveExecutionContext.gas_budget`. Other `GasBudget`
                // fields (gas_price, estimated_fee) are left at the
                // preparer's defaults; v1 charges no fee.
                t.gas_budget.max_gas_units = resolved_gas;
                t
            }
            Err(e) => {
                error!(error = %e, "PTB task preparation failed");
                return setu_api::MovePtbResponse {
                    event_id: String::new(),
                    success: false,
                    error: Some(format!("Task preparation failed: {}", e)),
                    code: None,
                };
            }
        };

        // 6. Route to a solver.
        let solver_id = match router_manager.route_any() {
            Ok(id) => id,
            Err(e) => {
                error!(error = %e, "No solver available for PTB");
                return setu_api::MovePtbResponse {
                    event_id: String::new(),
                    success: false,
                    error: Some(format!("No solver available: {}", e)),
                    code: None,
                };
            }
        };

        // 7. Execute via TeeExecutor.
        let call_id = format!("move-ptb-{}", vlc_time);
        match tee_executor.execute_solver_inline_batch(
            &call_id, &solver_id, solver_task, vec![],
        ).await {
            Ok((result_event, execution_time_us, events_processed)) => {
                let event_id = result_event.id.clone();
                let exec_result = result_event.execution_result.as_ref();
                let success = exec_result.map(|r| r.success).unwrap_or(false);
                let error = if success {
                    None
                } else {
                    exec_result.and_then(|r| r.message.clone())
                };

                // Stage to speculative overlay so the client can immediately
                // read-your-writes from this validator. CF finalize will
                // apply the canonical state via apply_committed_events.
                if success {
                    if let Some(r) = result_event.execution_result.as_ref() {
                        let shared = state_provider.shared_state_manager();
                        if let Err(e) = shared.stage_overlay(
                            &result_event.id,
                            SubnetId::ROOT,
                            &r.state_changes,
                        ) {
                            error!(
                                event_id = %result_event.id,
                                error = %e,
                                "PTB state_change has malformed key; overlay stage skipped"
                            );
                        }
                    }
                }

                tee_executor.spawn_post_execution(
                    call_id, result_event, execution_time_us, events_processed,
                );

                info!(
                    event_id = %event_id,
                    solver_id = %solver_id,
                    "PTB executed"
                );

                setu_api::MovePtbResponse {
                    event_id,
                    success,
                    error,
                    code: None,
                }
            }
            Err(e) => {
                error!(error = %e, "PTB TEE execution failed");
                setu_api::MovePtbResponse {
                    event_id: String::new(),
                    success: false,
                    error: Some(format!("Execution failed: {}", e)),
                    code: None,
                }
            }
        }
    }
}
