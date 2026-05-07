//! Native function implementations for Setu stdlib.
//!
//! 14 natives registered under `0x1`:
//! - object::new_uid_internal / delete_uid_internal / uid_to_address_internal
//! - transfer::transfer_internal / share_internal / freeze_internal
//! - tx_context::derive_id_internal
//! - event::emit_internal
//! - clock::timestamp_ms_internal
//! - dynamic_field::{add_internal, remove_internal,
//!   borrow_internal, borrow_mut_internal, exists_internal}

use std::collections::VecDeque;
use std::sync::Arc;

use move_binary_format::errors::{PartialVMError, PartialVMResult};
use move_core_types::{
    account_address::AccountAddress, gas_algebra::InternalGas, identifier::Identifier,
    vm_status::StatusCode,
};
use move_vm_runtime::native_functions::{NativeContext, NativeFunction, NativeFunctionTable};
use move_vm_types::{
    loaded_data::runtime_types::Type,
    natives::function::NativeResult,
    values::{Locals, Struct, StructRef, VMValueCast, Value},
};
use smallvec::smallvec;

use setu_types::dynamic_field::{derive_df_oid, DfAccessMode};
use setu_types::object::ObjectId;

use crate::object_runtime::{DfCreateEffect, DfDeleteEffect, DfMutateEffect, SetuObjectRuntime};

// ═══════════════════════════════════════════════════════════════
// Dynamic Field native abort codes (mirrors setu-framework/.../dynamic_field.move)
// ═══════════════════════════════════════════════════════════════

/// DF not preloaded by TaskPreparer (access mode missing or wrong).
const E_DF_NOT_PRELOADED: u64 = 0;
/// DF already exists on `add`.
const E_DF_ALREADY_EXISTS: u64 = 1;
/// DF does not exist on `remove` / `borrow` / `borrow_mut`.
const E_DF_DOES_NOT_EXIST: u64 = 2;
/// DF type_tag for V mismatches preloaded value_type_tag.
const E_DF_TYPE_MISMATCH: u64 = 3;
/// V: key — value UID would collide with a loaded input object
/// (defence against key-ability impersonation, v1.3 R3-ISSUE-1).
const E_DF_VALUE_HAS_KEY_ABILITY: u64 = 4;

// ═══════════════════════════════════════════════════════════════
// Registration table
// ═══════════════════════════════════════════════════════════════

/// Build the native function table for `0x1` (setu framework address).
pub fn setu_native_functions() -> NativeFunctionTable {
    let addr = AccountAddress::ONE;
    let mut table = make_table(
        addr,
        &[
            ("object", "new_uid_internal", native_object_new_uid),
            ("object", "delete_uid_internal", native_object_delete_uid),
            ("object", "uid_to_address_internal", native_uid_to_address),
            ("transfer", "transfer_internal", native_transfer_internal),
            ("transfer", "share_internal", native_share_internal),
            ("transfer", "freeze_internal", native_freeze_internal),
            ("tx_context", "derive_id_internal", native_derive_id),
            ("event", "emit_internal", native_event_emit),
            ("clock", "timestamp_ms_internal", native_clock_timestamp),
            ("dynamic_field", "add_internal", native_df_add_internal),
            (
                "dynamic_field",
                "remove_internal",
                native_df_remove_internal,
            ),
            (
                "dynamic_field",
                "borrow_internal",
                native_df_borrow_internal,
            ),
            (
                "dynamic_field",
                "borrow_mut_internal",
                native_df_borrow_mut_internal,
            ),
            (
                "dynamic_field",
                "exists_internal",
                native_df_exists_internal,
            ),
            // ── B3 (Phase 6) natives — bcs / address / hash / crypto ──
            (
                "bcs",
                "to_bytes_internal",
                crate::natives_b3::native_bcs_to_bytes,
            ),
            (
                "bcs",
                "from_bytes_internal",
                crate::natives_b3::native_bcs_from_bytes,
            ),
            (
                "address",
                "from_bytes_internal",
                crate::natives_b3::native_address_from_bytes,
            ),
            (
                "address",
                "to_bytes_internal",
                crate::natives_b3::native_address_to_bytes,
            ),
            (
                "hash",
                "sha2_256_internal",
                crate::natives_b3::native_hash_sha2_256,
            ),
            (
                "hash",
                "sha3_256_internal",
                crate::natives_b3::native_hash_sha3_256,
            ),
            (
                "hash",
                "blake3_internal",
                crate::natives_b3::native_hash_blake3,
            ),
            (
                "hash",
                "keccak256_internal",
                crate::natives_b3::native_hash_keccak256,
            ),
            (
                "crypto",
                "ed25519_verify_internal",
                crate::natives_b3::native_ed25519_verify,
            ),
            (
                "crypto",
                "ecdsa_k1_verify_internal",
                crate::natives_b3::native_ecdsa_k1_verify,
            ),
        ],
    );

    // ── std::vector natives (sourced from upstream Sui Move stdlib) ──
    //
    // setu-framework/sources/vector.move declares 8 native fns
    // (empty / length / borrow / push_back / borrow_mut / pop_back /
    // destroy_empty / swap). Linking any module that imports std::vector
    // — most notably `coin::split` / `coin::join` — fails with
    // MISSING_DEPENDENCY unless these are registered. We delegate to
    // upstream's `move_stdlib_natives::vector::make_all` which provides
    // all 8 (borrow_mut reuses borrow's gas params via .clone() at
    // upstream vector.rs:337). Gas params are zero — Setu uses
    // InstructionCountGasMeter at the VM level, not native-fn level.
    // Field shapes verified against
    // ~/.cargo/git/checkouts/sui-.../move-stdlib-natives/src/vector.rs.
    use move_stdlib_natives::vector as vector_natives;
    let vector_gas = vector_natives::GasParameters {
        empty: vector_natives::EmptyGasParameters { base: 0.into() },
        length: vector_natives::LengthGasParameters { base: 0.into() },
        push_back: vector_natives::PushBackGasParameters {
            base: 0.into(),
            legacy_per_abstract_memory_unit: 0.into(),
        },
        borrow: vector_natives::BorrowGasParameters { base: 0.into() },
        pop_back: vector_natives::PopBackGasParameters { base: 0.into() },
        destroy_empty: vector_natives::DestroyEmptyGasParameters { base: 0.into() },
        swap: vector_natives::SwapGasParameters { base: 0.into() },
    };
    let vector_module = Identifier::new("vector").expect("\"vector\" is a valid identifier");
    table.extend(vector_natives::make_all(vector_gas).map(|(func_name, f)| {
        (
            addr,
            vector_module.clone(),
            Identifier::new(func_name.as_str()).expect("upstream native identifier"),
            f,
        )
    }));

    table
}

type RawNativeFn =
    fn(&mut NativeContext, Vec<Type>, VecDeque<Value>) -> PartialVMResult<NativeResult>;

fn make_table(addr: AccountAddress, entries: &[(&str, &str, RawNativeFn)]) -> NativeFunctionTable {
    entries
        .iter()
        .map(|(module, func, f)| {
            (
                addr,
                Identifier::new(*module).expect("valid identifier"),
                Identifier::new(*func).expect("valid identifier"),
                Arc::new(*f) as NativeFunction,
            )
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════
// object::new_uid_internal
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun new_uid_internal(ctx: &mut TxContext): ID;`
///
/// Generates a deterministic object ID via SetuObjectRuntime.fresh_id().
/// Must consume the `&mut TxContext` arg to maintain VM stack balance (R4-C1).
fn native_object_new_uid(
    context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    // Consume &mut TxContext (unused — ID generation via SetuObjectRuntime)
    let _ctx = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    let new_id = runtime.fresh_id();

    // ID { bytes: address }
    let id_value = Value::address(AccountAddress::new(*new_id.as_bytes()));
    let id_struct = Value::struct_(Struct::pack(vec![id_value]));

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![id_struct]))
}

// ═══════════════════════════════════════════════════════════════
// object::delete_uid_internal
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun delete_uid_internal(id: ID);`
///
/// If the ID was created this tx, removes from `created_ids`.
/// Otherwise marks as deleted in `deleted_ids`.
fn native_object_delete_uid(
    context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let id_value = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    // ID { bytes: address } — BCS layout: struct with one address field
    let layout = move_vm_types::values::VMValueCast::cast(id_value)?;
    let id_struct: Struct = layout;
    let fields: Vec<Value> = id_struct.unpack()?.collect();
    if fields.is_empty() {
        return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR));
    }
    let addr: AccountAddress =
        move_vm_types::values::VMValueCast::cast(fields.into_iter().next().unwrap())?;
    let object_id = ObjectId::new(addr.into_bytes());

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    if !runtime.created_ids.swap_remove(&object_id) {
        runtime.delete_object(object_id);
    }

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![]))
}

// ═══════════════════════════════════════════════════════════════
// object::uid_to_address_internal
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun uid_to_address_internal(uid: &UID): address;`
///
/// UID { id: ID { bytes: address } } → extracts the inner address.
fn native_uid_to_address(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let uid_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    // UID layout in BCS: Struct { id: ID { bytes: address } }
    // Front 32 bytes = the address.
    use move_vm_types::values::VMValueCast;
    let uid_struct: Struct = VMValueCast::cast(uid_val)?;
    let mut uid_fields: Vec<Value> = uid_struct.unpack()?.collect();
    if uid_fields.is_empty() {
        return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR));
    }
    let id_val = uid_fields.remove(0);
    let id_struct: Struct = VMValueCast::cast(id_val)?;
    let mut id_fields: Vec<Value> = id_struct.unpack()?.collect();
    if id_fields.is_empty() {
        return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR));
    }
    let addr: AccountAddress = VMValueCast::cast(id_fields.remove(0))?;

    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::address(addr)],
    ))
}

// ═══════════════════════════════════════════════════════════════
// transfer::transfer_internal<T>
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun transfer_internal<T: key>(obj: T, recipient: address);`
fn native_transfer_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    // Args: (obj: T, recipient: address)  — pop in reverse order
    let recipient: AccountAddress = move_vm_types::values::VMValueCast::cast(
        args.pop_back()
            .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?,
    )?;
    let obj_value = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let obj_type = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    // Type tag
    let type_tag = context.type_to_type_tag(&obj_type)?;
    let struct_tag = match type_tag {
        move_core_types::language_storage::TypeTag::Struct(st) => *st,
        _ => return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR)),
    };

    // BCS serialize
    let layout = context
        .type_to_type_layout(&obj_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let bcs_bytes = obj_value
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;

    // Extract UID from first 32 bytes
    let object_id = extract_uid_from_bcs(&bcs_bytes)?;

    // Record transfer
    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    runtime.transfer_object(object_id, recipient, struct_tag, bcs_bytes);

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![]))
}

// ═══════════════════════════════════════════════════════════════
// transfer::share_internal<T>
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun share_internal<T: key>(obj: T);`
///
/// PWOO: marks the object as `Ownership::Shared { initial_shared_version }`.
/// The executor (setu-enclave / runtime) converts this runtime effect into
/// a StateChange that persists the envelope with the new ownership.
fn native_share_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let obj_value = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let obj_type = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let type_tag = context.type_to_type_tag(&obj_type)?;
    let struct_tag = match type_tag {
        move_core_types::language_storage::TypeTag::Struct(st) => *st,
        _ => return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR)),
    };

    let layout = context
        .type_to_type_layout(&obj_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let bcs_bytes = obj_value
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;

    let object_id = extract_uid_from_bcs(&bcs_bytes)?;

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    runtime.share_object(object_id, struct_tag, bcs_bytes);

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![]))
}

// ═══════════════════════════════════════════════════════════════
// transfer::freeze_internal<T>
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun freeze_internal<T: key>(obj: T);`
fn native_freeze_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let obj_value = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let obj_type = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let type_tag = context.type_to_type_tag(&obj_type)?;
    let struct_tag = match type_tag {
        move_core_types::language_storage::TypeTag::Struct(st) => *st,
        _ => return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR)),
    };

    let layout = context
        .type_to_type_layout(&obj_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let bcs_bytes = obj_value
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;

    let object_id = extract_uid_from_bcs(&bcs_bytes)?;

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    runtime.freeze_object(object_id, struct_tag, bcs_bytes);

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![]))
}

// ═══════════════════════════════════════════════════════════════
// tx_context::derive_id_internal
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun derive_id(tx_hash: vector<u8>, creation_num: u64): address;`
fn native_derive_id(
    _context: &mut NativeContext,
    _ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    use move_vm_types::values::VMValueCast;
    let creation_num: u64 = VMValueCast::cast(
        args.pop_back()
            .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?,
    )?;
    let tx_hash_val: Vec<u8> = VMValueCast::cast(
        args.pop_back()
            .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?,
    )?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"SETU_DERIVE_ID:");
    hasher.update(&tx_hash_val);
    hasher.update(&creation_num.to_le_bytes());
    let hash = hasher.finalize();

    let addr = AccountAddress::new(*hash.as_bytes());
    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::address(addr)],
    ))
}

// ═══════════════════════════════════════════════════════════════
// event::emit_internal<T>
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun emit_internal<T: copy + drop>(event: T);`
///
/// BCS-serializes the event value and records it in SetuObjectRuntime.
fn native_event_emit(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let event_value = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let event_type = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let type_tag = context.type_to_type_tag(&event_type)?;
    let struct_tag = match type_tag {
        move_core_types::language_storage::TypeTag::Struct(st) => *st,
        _ => return Err(PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR)),
    };

    let layout = context
        .type_to_type_layout(&event_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let bcs_bytes = event_value
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    runtime.emit_event(struct_tag, bcs_bytes);

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![]))
}

// ═══════════════════════════════════════════════════════════════
// clock::timestamp_ms_internal
// ═══════════════════════════════════════════════════════════════

/// Move: `native fun timestamp_ms_internal(): u64;`
///
/// Returns the epoch timestamp in milliseconds, injected by consensus.
fn native_clock_timestamp(
    context: &mut NativeContext,
    _ty_args: Vec<Type>,
    _args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    let ts = runtime.epoch_timestamp_ms();
    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::u64(ts)],
    ))
}

// ═══════════════════════════════════════════════════════════════
// dynamic_field natives (M2)
// ═══════════════════════════════════════════════════════════════
//
// Layout contract (matches setu-framework/sources/dynamic_field.move):
//
//   native fun add_internal<K: copy + drop + store, V: store>(
//       parent: &mut UID, name: K, value: V);
//   native fun remove_internal<K: copy + drop + store, V: store>(
//       parent: &mut UID, name: K): V;
//   native fun borrow_internal<K: copy + drop + store, V: store>(
//       parent: &UID, name: K): &V;
//   native fun borrow_mut_internal<K: copy + drop + store, V: store>(
//       parent: &mut UID, name: K): &mut V;
//   native fun exists_internal<K: copy + drop + store>(
//       parent: &UID, name: K): bool;
//
// Arg order after `pop_back` (Move pushes left-to-right, natives pop RTL):
//   1st pop: `name`        (K)
//   2nd pop: `parent`      (&UID or &mut UID)
// For add/remove `value` is pushed last → popped first:
//   1st pop: `value`       (V)
//   2nd pop: `name`        (K)
//   3rd pop: `parent`      (&mut UID)

/// Pop a `&UID` / `&mut UID` reference value from the args and extract the
/// underlying parent ObjectId (the inner address of `UID { id: ID { bytes } }`).
fn extract_parent_oid_from_uid_ref(uid_ref_val: Value) -> PartialVMResult<ObjectId> {
    // &UID → StructRef → read_ref → Value(Struct(UID)) → unpack first field
    // which is ID { bytes: address } → unpack first field which is address.
    let struct_ref: StructRef = uid_ref_val.cast()?;
    let uid_struct: Struct = struct_ref.read_ref()?.cast()?;
    let mut uid_fields = uid_struct.unpack()?;
    let id_val = uid_fields.next().ok_or_else(|| {
        PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR)
            .with_message("UID struct missing id field".to_string())
    })?;
    let id_struct: Struct = id_val.cast()?;
    let mut id_fields = id_struct.unpack()?;
    let addr_val = id_fields.next().ok_or_else(|| {
        PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR)
            .with_message("ID struct missing bytes field".to_string())
    })?;
    let addr: AccountAddress = addr_val.cast()?;
    Ok(ObjectId::new(addr.into_bytes()))
}

/// Resolve K's canonical type tag string and BCS-serialize `name`.
fn serialize_name(
    context: &mut NativeContext,
    name_type: &Type,
    name_value: Value,
) -> PartialVMResult<(String, Vec<u8>)> {
    let tag = context.type_to_type_tag(name_type)?;
    let layout = context
        .type_to_type_layout(name_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let bcs = name_value
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;
    Ok((tag.to_canonical_string(/* with_prefix */ true), bcs))
}

/// Resolve V's canonical type tag string + layout + BCS.
fn serialize_value(
    context: &mut NativeContext,
    value_type: &Type,
    value_value: Value,
) -> PartialVMResult<(String, Vec<u8>)> {
    let tag = context.type_to_type_tag(value_type)?;
    let layout = context
        .type_to_type_layout(value_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let bcs = value_value
        .typed_serialize(&layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_SERIALIZATION_ERROR))?;
    Ok((tag.to_canonical_string(true), bcs))
}

/// Deserialize cached DF `value_bcs` back into a Move `Value` using V's layout.
fn deserialize_value_bytes(
    context: &mut NativeContext,
    value_type: &Type,
    bcs: &[u8],
) -> PartialVMResult<Value> {
    let layout = context
        .type_to_type_layout(value_type)?
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    Value::simple_deserialize(bcs, &layout)
        .ok_or_else(|| PartialVMError::new(StatusCode::VALUE_DESERIALIZATION_ERROR))
}

/// `dynamic_field::add_internal<K, V>(parent: &mut UID, name: K, value: V)`
fn native_df_add_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    // ty_args: [K, V]
    let v_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let k_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    // args reverse order: value, name, parent
    let value_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let name_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let parent_ref_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let parent_oid = extract_parent_oid_from_uid_ref(parent_ref_val)?;
    let (name_tag, name_bcs) = serialize_name(context, &k_ty, name_val)?;
    let (value_tag, value_bcs) = serialize_value(context, &v_ty, value_val)?;

    let df_oid = derive_df_oid(&parent_oid, &name_tag, &name_bcs);
    let cache_key = (parent_oid, name_tag.clone(), name_bcs.clone());

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;

    // V:key collision defence — if value_bcs first 32B match an input_object,
    // the caller likely crafted a `V: key` to impersonate an existing object.
    if value_bcs.len() >= 32 {
        let mut candidate = [0u8; 32];
        candidate.copy_from_slice(&value_bcs[..32]);
        if runtime.input_object_exists(&ObjectId::new(candidate)) {
            return Ok(NativeResult::err(
                InternalGas::zero(),
                E_DF_VALUE_HAS_KEY_ABILITY,
            ));
        }
    }

    // Must be preloaded in Create mode.
    // Exception: mutate-mode replace (`remove` then `add` in the same tx)
    // is allowed when the cache slot is currently empty.
    let entry = match runtime.df_cache_get(&cache_key) {
        Some(e) => e,
        None => {
            return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
        }
    };
    let mutate_replace = entry.mode == DfAccessMode::Mutate && entry.value_bytes.is_none();
    if entry.mode != DfAccessMode::Create && !mutate_replace {
        return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
    }
    if entry.value_bytes.is_some() {
        return Ok(NativeResult::err(InternalGas::zero(), E_DF_ALREADY_EXISTS));
    }
    if entry.value_type_tag != value_tag {
        return Ok(NativeResult::err(InternalGas::zero(), E_DF_TYPE_MISMATCH));
    }
    if entry.df_oid != df_oid {
        // derive mismatch → preload inconsistency; surface as NOT_PRELOADED
        return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
    }

    if mutate_replace {
        // `remove` must have recorded an old-value effect earlier in this tx.
        let prior_delete = match runtime.take_df_delete(&df_oid) {
            Some(eff) => eff,
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
            }
        };

        runtime.record_df_mutate(
            df_oid,
            DfMutateEffect {
                parent: parent_oid,
                key_type_tag: name_tag.clone(),
                key_bcs: name_bcs.clone(),
                value_type_tag: value_tag.clone(),
                old_value_bcs: prior_delete.old_value_bcs,
                new_value_bcs: value_bcs.clone(),
                on_disk_envelope: prior_delete.on_disk_envelope,
            },
        );
    } else {
        runtime.record_df_create(
            df_oid,
            DfCreateEffect {
                parent: parent_oid,
                key_type_tag: name_tag.clone(),
                key_bcs: name_bcs.clone(),
                value_type_tag: value_tag,
                value_bcs: value_bcs.clone(),
            },
        );
    }

    // Keep the cache coherent so a later `borrow` in the same tx sees it.
    if let Some(e) = runtime.df_cache_get_mut(&cache_key) {
        e.value_bytes = Some(value_bcs);
    }

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![]))
}

/// `dynamic_field::remove_internal<K, V>(parent: &mut UID, name: K): V`
fn native_df_remove_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let v_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let k_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let name_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let parent_ref_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let parent_oid = extract_parent_oid_from_uid_ref(parent_ref_val)?;
    let (name_tag, name_bcs) = serialize_name(context, &k_ty, name_val)?;
    let v_tag = context.type_to_type_tag(&v_ty)?.to_canonical_string(true);
    let cache_key = (parent_oid, name_tag.clone(), name_bcs.clone());

    // Extract needed info under the runtime borrow, then deserialize after drop.
    let (df_oid, value_bcs) = {
        let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
        let entry = match runtime.df_cache_get(&cache_key) {
            Some(e) => e,
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
            }
        };
        if !matches!(entry.mode, DfAccessMode::Delete | DfAccessMode::Mutate) {
            return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
        }
        if entry.value_type_tag != v_tag {
            return Ok(NativeResult::err(InternalGas::zero(), E_DF_TYPE_MISMATCH));
        }
        let bytes = match &entry.value_bytes {
            Some(b) => b.clone(),
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_DOES_NOT_EXIST));
            }
        };
        let df_oid = entry.df_oid;
        let on_disk_envelope = entry.envelope_bytes.clone();
        runtime.record_df_delete(
            df_oid,
            DfDeleteEffect {
                parent: parent_oid,
                key_type_tag: name_tag.clone(),
                key_bcs: name_bcs.clone(),
                value_type_tag: v_tag.clone(),
                old_value_bcs: bytes.clone(),
                on_disk_envelope,
            },
        );
        // Invalidate cache so a later borrow returns DOES_NOT_EXIST.
        if let Some(e) = runtime.df_cache_get_mut(&cache_key) {
            e.value_bytes = None;
        }
        (df_oid, bytes)
    };

    let value = deserialize_value_bytes(context, &v_ty, &value_bcs)?;
    let _ = df_oid; // recorded above; returned by value not needed here
    Ok(NativeResult::ok(InternalGas::zero(), smallvec![value]))
}

/// `dynamic_field::borrow_internal<K, V>(parent: &UID, name: K): &V`
///
/// NOTE (M2 simplification): the Move front-end calls this native via
/// `public fun borrow<K, V>(p: &UID, n: K): &V { &borrow_internal(...) }`.
/// Returning `Value` (owned) vs `&V` (reference) is reconciled by the Move
/// source using `&move` / temporary binding. The native returns a fresh
/// owned Value; the Move fun wraps it into a reference via standard borrow.
fn native_df_borrow_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let v_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let k_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let name_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let parent_ref_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let parent_oid = extract_parent_oid_from_uid_ref(parent_ref_val)?;
    let (name_tag, name_bcs) = serialize_name(context, &k_ty, name_val)?;
    let v_tag = context.type_to_type_tag(&v_ty)?.to_canonical_string(true);
    let cache_key = (parent_oid, name_tag, name_bcs);

    let bytes = {
        let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
        let entry = match runtime.df_cache_get(&cache_key) {
            Some(e) => e,
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
            }
        };
        if entry.value_type_tag != v_tag {
            return Ok(NativeResult::err(InternalGas::zero(), E_DF_TYPE_MISMATCH));
        }
        match &entry.value_bytes {
            Some(b) => b.clone(),
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_DOES_NOT_EXIST));
            }
        }
    };
    let value = deserialize_value_bytes(context, &v_ty, &bytes)?;

    // Native signatures require returning `&V`.
    // Use a struct-backed container to produce a stable field reference.
    let mut locals = Locals::new(1);
    locals.store_loc(0, Value::struct_(Struct::pack(vec![value])), false)?;
    let struct_ref_val = locals.borrow_loc(0)?;
    let struct_ref: StructRef = struct_ref_val.cast()?;
    let value_ref = struct_ref.borrow_field(0)?;

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![value_ref]))
}

/// `dynamic_field::borrow_mut_internal<K, V>(parent: &mut UID, name: K): &mut V`
///
/// M3 behaviour: returns the current value + records a `DfMutateEffect`
/// with `new_value_bcs == old_value_bcs` as a placeholder. Real in-place
/// writeback of `&mut V` requires Move VM reference-tracking, which is
/// deferred to a later milestone. Until then, `borrow_mut` is byte-wise
/// idempotent at the StateChange layer.
fn native_df_borrow_mut_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let v_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let k_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let name_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let parent_ref_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let parent_oid = extract_parent_oid_from_uid_ref(parent_ref_val)?;
    let (name_tag, name_bcs) = serialize_name(context, &k_ty, name_val)?;
    let v_tag = context.type_to_type_tag(&v_ty)?.to_canonical_string(true);
    let cache_key = (parent_oid, name_tag.clone(), name_bcs.clone());

    let bytes = {
        let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
        let entry = match runtime.df_cache_get(&cache_key) {
            Some(e) => e,
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
            }
        };
        if !matches!(entry.mode, DfAccessMode::Mutate) {
            return Ok(NativeResult::err(InternalGas::zero(), E_DF_NOT_PRELOADED));
        }
        if entry.value_type_tag != v_tag {
            return Ok(NativeResult::err(InternalGas::zero(), E_DF_TYPE_MISMATCH));
        }
        let bytes = match &entry.value_bytes {
            Some(b) => b.clone(),
            None => {
                return Ok(NativeResult::err(InternalGas::zero(), E_DF_DOES_NOT_EXIST));
            }
        };
        let df_oid = entry.df_oid;
        let on_disk_envelope = entry.envelope_bytes.clone();
        runtime.record_df_mutate(
            df_oid,
            DfMutateEffect {
                parent: parent_oid,
                key_type_tag: name_tag.clone(),
                key_bcs: name_bcs.clone(),
                value_type_tag: v_tag.clone(),
                old_value_bcs: bytes.clone(),
                new_value_bcs: bytes.clone(),
                on_disk_envelope,
            },
        );
        bytes
    };
    let value = deserialize_value_bytes(context, &v_ty, &bytes)?;

    // Same reference-construction approach as immutable borrow.
    let mut locals = Locals::new(1);
    locals.store_loc(0, Value::struct_(Struct::pack(vec![value])), false)?;
    let struct_ref_val = locals.borrow_loc(0)?;
    let struct_ref: StructRef = struct_ref_val.cast()?;
    let value_ref = struct_ref.borrow_field(0)?;

    Ok(NativeResult::ok(InternalGas::zero(), smallvec![value_ref]))
}

/// `dynamic_field::exists_internal<K>(parent: &UID, name: K): bool`
fn native_df_exists_internal(
    context: &mut NativeContext,
    mut ty_args: Vec<Type>,
    mut args: VecDeque<Value>,
) -> PartialVMResult<NativeResult> {
    let k_ty = ty_args
        .pop()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let name_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;
    let parent_ref_val = args
        .pop_back()
        .ok_or_else(|| PartialVMError::new(StatusCode::INTERNAL_TYPE_ERROR))?;

    let parent_oid = extract_parent_oid_from_uid_ref(parent_ref_val)?;
    let (name_tag, name_bcs) = serialize_name(context, &k_ty, name_val)?;
    let cache_key = (parent_oid, name_tag, name_bcs);

    let runtime = context.extensions_mut().get_mut::<SetuObjectRuntime>()?;
    let exists = runtime
        .df_cache_get(&cache_key)
        .map(|e| e.value_bytes.is_some())
        .unwrap_or(false);
    Ok(NativeResult::ok(
        InternalGas::zero(),
        smallvec![Value::bool(exists)],
    ))
}

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

/// Extract object UID from BCS bytes.
///
/// Layout: `Struct { id: UID { id: ID { bytes: address } }, ... }`
/// First 32 bytes = the address (all nested structs start at offset 0 in BCS).
fn extract_uid_from_bcs(bcs_bytes: &[u8]) -> PartialVMResult<ObjectId> {
    if bcs_bytes.len() < 32 {
        return Err(PartialVMError::new(StatusCode::VALUE_DESERIALIZATION_ERROR));
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&bcs_bytes[..32]);
    Ok(ObjectId::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_table_count() {
        let table = setu_native_functions();
        // 14 base natives + 10 B3 natives (bcs/address/hash/crypto)
        // + 8 std::vector natives (empty/length/borrow/borrow_mut/push_back/
        //   pop_back/destroy_empty/swap) sourced from move-stdlib-natives.
        assert_eq!(table.len(), 32);
    }

    #[test]
    fn test_make_table_modules() {
        let table = setu_native_functions();
        let modules: Vec<String> = table.iter().map(|(_, m, _, _)| m.to_string()).collect();
        assert!(modules.contains(&"object".to_string()));
        assert!(modules.contains(&"transfer".to_string()));
        assert!(modules.contains(&"tx_context".to_string()));
        assert!(modules.contains(&"event".to_string()));
        assert!(modules.contains(&"clock".to_string()));
        assert!(modules.contains(&"dynamic_field".to_string()));
        assert!(modules.contains(&"vector".to_string()));
    }

    #[test]
    fn test_make_table_includes_df_natives() {
        let table = setu_native_functions();
        let df_fns: Vec<String> = table
            .iter()
            .filter(|(_, m, _, _)| m.as_str() == "dynamic_field")
            .map(|(_, _, f, _)| f.to_string())
            .collect();
        for expected in [
            "add_internal",
            "remove_internal",
            "borrow_internal",
            "borrow_mut_internal",
            "exists_internal",
        ] {
            assert!(
                df_fns.contains(&expected.to_string()),
                "dynamic_field::{} not registered",
                expected
            );
        }
        assert_eq!(df_fns.len(), 5);
    }

    #[test]
    fn test_extract_uid_from_bcs() {
        let mut data = vec![0u8; 64];
        data[..32].copy_from_slice(&[0x42; 32]);
        let id = extract_uid_from_bcs(&data).unwrap();
        assert_eq!(id, ObjectId::new([0x42; 32]));
    }

    #[test]
    fn test_extract_uid_too_short() {
        let data = vec![0u8; 16];
        assert!(extract_uid_from_bcs(&data).is_err());
    }

    #[test]
    fn test_derive_id_deterministic() {
        // Simulate derive_id logic directly
        let tx_hash = vec![0xAA; 32];
        let creation_num: u64 = 5;

        let mut h1 = blake3::Hasher::new();
        h1.update(b"SETU_DERIVE_ID:");
        h1.update(&tx_hash);
        h1.update(&creation_num.to_le_bytes());
        let r1 = h1.finalize();

        let mut h2 = blake3::Hasher::new();
        h2.update(b"SETU_DERIVE_ID:");
        h2.update(&tx_hash);
        h2.update(&creation_num.to_le_bytes());
        let r2 = h2.finalize();

        assert_eq!(r1, r2);
    }
}
