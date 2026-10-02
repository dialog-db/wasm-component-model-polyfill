// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The error-context built-ins.
//!
//! An error context is one store-wide record: the debug message a
//! guest gave it, and the count of the guest handles that name it. A
//! handle is an entry of the error-context kind in the handle table
//! of the instance that holds it. Three canonical built-ins work on
//! them:
//!
//! - `error-context.new` reads the debug message out of the guest's
//!   memory, in the string encoding its canon options declare, and
//!   keeps it exactly as written, the empty string included. That is
//!   Wasmtime's choice; the reference lets the host replace the
//!   message with an empty one. The record starts with a count of
//!   one, and the built-in returns the index of the new entry. A
//!   message that leaves the memory fails with the bounds checks
//!   every string lift makes. The record counts against the cap on
//!   the store's live records, so a record past the cap fails with
//!   the full-table cause.
//! - `error-context.debug-message` writes the message into the
//!   guest's memory through the `realloc` its canon options name, in
//!   their string encoding, and stores the pointer and the length at
//!   the address the guest passes. It first checks that the eight
//!   bytes at that address lie inside the memory, and only then calls
//!   `realloc`, which is Wasmtime's order. The reference calls
//!   `realloc` first and fails on the store, but no guest can tell
//!   the two apart: the trap ends the call before the allocation
//!   could be read.
//! - `error-context.drop` takes the entry away and subtracts one
//!   from the record's count. The record leaves the store when the
//!   count reaches zero.
//!
//! Each of the three first traps with the cannot-leave cause when the
//! instance's may-leave flag is clear, which is the case while a
//! `realloc` or a `post-return` of that instance runs. The two that
//! take a handle trap when the index names no entry, or an entry of
//! another kind, before they do anything else.
//!
//! A fused adapter imports one more, `error-context.transfer`, for an
//! error context in a parameter or a result of a call between two
//! components. It copies the handle: the sender keeps its entry, the
//! receiver gains one, and the record's count rises by one.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift_string, lower_str, write_pointer_pair};
use crate::concurrency::{ErrorContextId, InstanceId};
use crate::error::{AbiPosition, Error, ErrorContextCause, TaskCause};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CanonOptions, CoreSignature};
use crate::resource::{HandleLookupError, HandleTables, TableId};
use crate::runtime_layer::host_func;
use crate::runtime_layer::{
    AsContextMut, Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};
use crate::store::StoreContextInternalExt;
use crate::store::{StoreContext, StoreData};
use crate::types::{PrimitiveType, ValueType};

/// The bytes `error-context.debug-message` stores at the guest's
/// address: the pointer and the length of the message, each a `u32`.
const POINTER_PAIR_SIZE: usize = 8;

/// Build the `error-context.new` built-in. The guest passes the
/// pointer and the length of the debug message, and receives the
/// index of the new error context in its instance's handle table.
pub fn build_error_context_new<T: 'static>(
    store: &mut StoreContext<'_, T>,
    options: &CanonOptions,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    let options = Arc::new(options.clone());
    host_func(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, results| {
            let index = error_context_new(store_ctx, &options, &abi_state, &tables, args)?;
            results[0] = RuntimeVal::I32(index as i32);
            Ok(())
        },
    )
}

/// Build the `error-context.debug-message` built-in. The guest
/// passes the index of an error context and the address the pointer
/// and the length of its debug message are stored at.
pub fn build_error_context_debug_message<T: 'static>(
    store: &mut StoreContext<'_, T>,
    options: &CanonOptions,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    let options = Arc::new(options.clone());
    host_func(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, _results| {
            error_context_debug_message(store_ctx, &options, &abi_state, &tables, args)
        },
    )
}

/// Build the `error-context.drop` built-in for `instance`: the named
/// entry leaves the instance's handle table, and the record it named
/// loses one handle.
pub fn build_error_context_drop<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: usize,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    host_func(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |mut store_ctx, args, _results| {
            let index = arg_u32(args, 0)?;
            let (id, table) = calling_instance(&abi_state, instance)?;
            trap_if_cannot_leave(&abi_state, id, &mut store_ctx)?;
            let mut guard = lock_tables(&tables)?;
            let context = error_context_at(&guard, table, index)?;
            guard.remove(table, index);
            guard.tasks.release_error_context(context).map_err(trap)
        },
    )
}

/// Build the `error-context.transfer` intrinsic a fused adapter
/// imports, over `instances`, the component instance of each
/// error-context table of the component. The adapter calls it once
/// per error context in a parameter or a result, with the context's
/// index in the sender's table, the sender's table, and the
/// receiver's table, and receives the index in the receiver's table.
///
/// An error context is copied between components, not moved: the
/// sender's entry stays, and the receiver gains an entry of its own
/// over the same record, whose count rises by one. That is
/// Wasmtime's transfer. An entry of another kind fails with the
/// error-context cause, and a count past `u32::MAX` fails with the
/// reference-count cause and enters nothing.
pub fn build_error_context_transfer<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instances: Arc<[usize]>,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> crate::error::Result<RuntimeFunc> {
    let tables = store.internal().tables_handle();
    host_func(
        store.internal().runtime_mut(),
        core_func_type(signature),
        move |_store_ctx, args, results| {
            let index = arg_u32(args, 0)?;
            let source = error_context_table(&instances, &abi_state, arg_u32(args, 1)?)?;
            let destination = error_context_table(&instances, &abi_state, arg_u32(args, 2)?)?;
            let mut guard = lock_tables(&tables)?;
            let context = error_context_at(&guard, source, index)?;
            guard.tasks.retain_error_context(context).map_err(trap)?;
            let out = guard.insert_error_context(destination, context);
            results[0] = RuntimeVal::I32(out as i32);
            Ok(())
        },
    )
}

/// The handle table of the component instance whose error-context
/// table an adapter named by `table_index`.
fn error_context_table(
    instances: &[usize],
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    table_index: u32,
) -> anyhow::Result<TableId> {
    let instance = *instances.get(table_index as usize).ok_or_else(|| {
        anyhow!(
            "an adapter named error-context table {table_index}, which the component does not declare"
        )
    })?;
    let state = abi_state
        .lock()
        .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?;
    state.handle_tables.get(instance).copied().ok_or_else(|| {
        anyhow!(
            "error-context table {table_index} names component instance {instance}, which this instantiation does not hold"
        )
    })
}

/// The body of the `error-context.new` built-in: the index of the
/// new entry, or the trap.
fn error_context_new<T: 'static>(
    mut store_ctx: RuntimeContextMut<'_, StoreData<T>>,
    options: &Arc<CanonOptions>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    args: &[RuntimeVal],
) -> anyhow::Result<u32> {
    let pointer = arg_u32(args, 0)?;
    let length = arg_u32(args, 1)?;
    let (id, table) = calling_instance(abi_state, options.instance)?;
    trap_if_cannot_leave(abi_state, id, &mut store_ctx)?;

    let debug_message = {
        let (boundary_options, instance) =
            BoundaryInstance::resolve(options, abi_state, tables).map_err(trap)?;
        let scope = lock_tables(tables)?.tasks.current_scope();
        let mut ctx = BoundaryContext::new(
            store_ctx.as_context_mut(),
            boundary_options,
            instance,
            scope,
        );
        lift_string(
            &mut ctx,
            pointer as usize,
            length as usize,
            AbiPosition::Argument(0),
            &string_type(),
        )
        .map_err(trap)?
    };

    let mut guard = lock_tables(tables)?;
    let context = guard
        .tasks
        .insert_error_context(debug_message)
        .map_err(trap)?;
    Ok(guard.insert_error_context(table, context))
}

/// The body of the `error-context.debug-message` built-in.
fn error_context_debug_message<T: 'static>(
    mut store_ctx: RuntimeContextMut<'_, StoreData<T>>,
    options: &Arc<CanonOptions>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    args: &[RuntimeVal],
) -> anyhow::Result<()> {
    let index = arg_u32(args, 0)?;
    let address = arg_u32(args, 1)? as usize;
    let (id, table) = calling_instance(abi_state, options.instance)?;
    trap_if_cannot_leave(abi_state, id, &mut store_ctx)?;

    // The message is copied out of the record before the guest runs
    // again: the `realloc` below is guest code, and the record stays
    // the store's while it runs.
    let (debug_message, scope) = {
        let guard = lock_tables(tables)?;
        let context = error_context_at(&guard, table, index)?;
        let record = guard
            .tasks
            .error_context(context)
            .ok_or_else(|| anyhow!("an error-context handle named no record"))?;
        (record.debug_message.clone(), guard.tasks.current_scope())
    };

    let (boundary_options, instance) =
        BoundaryInstance::resolve(options, abi_state, tables).map_err(trap)?;
    let mut ctx = BoundaryContext::new(
        store_ctx.as_context_mut(),
        boundary_options,
        instance,
        scope,
    );
    // The address is checked before `realloc` runs, so a call that
    // fails here allocates nothing.
    if !ctx.in_bounds(address, POINTER_PAIR_SIZE) {
        return Err(trap(Error::ErrorContext(
            ErrorContextCause::DebugMessagePointerOutOfBounds,
        )));
    }
    let position = AbiPosition::Argument(1);
    let ty = string_type();
    let (pointer, units) = lower_str(&mut ctx, &debug_message, position, &ty).map_err(trap)?;
    write_pointer_pair(&mut ctx, address, pointer, units as usize, position, &ty).map_err(trap)
}

/// The error context the entry at `index` names, or the trap for an
/// index that names nothing or names an entry of another kind.
fn error_context_at(
    tables: &HandleTables,
    table: TableId,
    index: u32,
) -> anyhow::Result<ErrorContextId> {
    tables
        .error_context_from_handle(table, index)
        .map_err(|err| match err {
            HandleLookupError::NotAnErrorContext { index } => {
                trap(Error::ErrorContext(ErrorContextCause::NotAnErrorContext {
                    index,
                }))
            }
            other => anyhow!("{other}"),
        })
}

/// The type a debug message crosses as.
fn string_type() -> ValueType {
    ValueType::Primitive(PrimitiveType::String)
}

/// The store-wide identity of the component instance the translator
/// named for a built-in, with the handle table that instance keeps.
fn calling_instance(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
) -> anyhow::Result<(InstanceId, TableId)> {
    let state = abi_state
        .lock()
        .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?;
    let id = state.component_instances.get(instance).copied();
    let table = state.handle_tables.get(instance).copied();
    match (id, table) {
        (Some(id), Some(table)) => Ok((id, table)),
        _ => Err(anyhow!(
            "a built-in named component instance {instance}, which this instantiation does not hold"
        )),
    }
}

/// Refuse the built-in when the instance may not be left, which is
/// the case while a `realloc` or a `post-return` of that instance
/// runs. The flag is the core global the instance's adapters compile
/// against, so this reads what the generated code reads.
fn trap_if_cannot_leave(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: InstanceId,
    store: impl AsContextMut,
) -> anyhow::Result<()> {
    let flags = {
        let state = abi_state
            .lock()
            .map_err(|_| anyhow!("ABI runtime state lock poisoned"))?;
        state.flags_of(instance).cloned().ok_or_else(|| {
            anyhow!("a built-in named an instance with no may-leave flag of its own")
        })?
    };
    if flags.may_leave(store).map_err(|err| anyhow!("{err}"))? {
        return Ok(());
    }
    Err(trap(Error::Task(TaskCause::CannotLeave)))
}

/// Lock the store's handle tables and record state.
fn lock_tables(
    tables: &Arc<Mutex<HandleTables>>,
) -> anyhow::Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| anyhow!("resource handle tables lock poisoned"))
}

/// The trap a structured error becomes on its way to the guest. The
/// message is the error's own with its chain flattened into it,
/// because a trap crosses back into guest code as a string; the
/// conformance corpora match it by substring.
fn trap(error: Error) -> anyhow::Error {
    let mut message = error.to_string();
    let mut link = std::error::Error::source(&error);
    while let Some(source) = link {
        message.push_str(": ");
        message.push_str(&source.to_string());
        link = source.source();
    }
    anyhow!("{message}")
}

fn arg_u32(args: &[RuntimeVal], index: usize) -> anyhow::Result<u32> {
    match args.get(index) {
        Some(RuntimeVal::I32(value)) => Ok(*value as u32),
        _ => Err(anyhow!(
            "an error-context built-in expected an i32 argument"
        )),
    }
}
