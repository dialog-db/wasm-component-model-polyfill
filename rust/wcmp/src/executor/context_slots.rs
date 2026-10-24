// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `context.get` and `context.set` built-ins, over a cache of the
//! current thread's context slots in WebAssembly globals.
//!
//! A thread's two context slots live on its record in the store's task
//! tables. A guest built with a shadow stack in a context slot reads and
//! writes slot 0 around most of its functions, so a built-in that calls
//! the host for each read costs more than the guest's own work.
//!
//! Each store has one core instance, the context module, that holds the
//! slots of one thread in two mutable globals, with a third global that
//! says whether they are loaded. The built-ins are its exports. A read
//! or a write of a loaded slot touches the globals alone. A read or a
//! write while nothing is loaded first calls the host's `load`, which
//! copies the current thread's slots in and records that thread as the
//! owner of the globals.
//!
//! The current thread changes only while the host runs. So the store's
//! call hook, which the runtime layer calls at each crossing between the
//! host and guest code, keeps the owner's record current and unloads the
//! slots wherever another thread could run next:
//!
//! - When the guest calls a host function, the hook copies the slots to
//!   the owner's record, so host code that reads the record sees what the
//!   thread last wrote. The slots stay loaded.
//! - When the host function returns, the slots stay loaded where the owner
//!   is still the current thread: the guest that called the function goes
//!   on. Otherwise the hook writes them back and unloads them, as after a
//!   host function that switched threads, or after a stretch of guest code
//!   the function ran whose wait dropped.
//! - Before and after each stretch of guest code the host runs, the hook
//!   writes the slots back and unloads them. A thread that runs next loads
//!   its own.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::anyhow;

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, EntityType, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction, Module as Encoded,
    TypeSection, ValType,
};

use crate::concurrency::ThreadId;
use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::resource::HandleTables;
use crate::runtime_layer::{
    AsContextMut, CallHook, Caller as RuntimeCaller, Extern as RuntimeExtern, Func as RuntimeFunc,
    FuncType, GeneratedModules, Global as RuntimeGlobal, Imports, Instance as RuntimeInstance,
    Module as RuntimeModule, RuntimeError, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
    instantiate, substrate_failure,
};
use crate::store::{StoreContext, StoreContextInternalExt, StoreData};

/// How many context slots a thread has.
const SLOTS: usize = 2;

/// The index of the global that says whether the slots are loaded.
const LOADED: u32 = SLOTS as u32;

/// The context module of a store: the instance whose exports are the
/// `context.get` and `context.set` built-ins of every component the
/// store instantiates. Clones share the instance and its cache.
#[derive(Clone)]
pub struct ContextSlots {
    instance: RuntimeInstance,
}

/// The globals of the context module and the thread whose slots they
/// hold, if they hold any.
#[derive(Clone, Copy)]
struct Cache {
    slots: [RuntimeGlobal; SLOTS],
    loaded: RuntimeGlobal,
    owner: Option<ThreadId>,
}

/// The cache, shared by `load` and the call hook. It is empty only while
/// the context module is instantiated.
type SharedCache = Arc<Mutex<Option<Cache>>>;

impl ContextSlots {
    /// The context module of `store`, instantiated and installed the
    /// first time a store needs it.
    pub async fn of_store<T: 'static>(store: &mut StoreContext<'_, T>) -> Result<Self> {
        if let Some(slots) = store.internal().context_slots() {
            return Ok(slots);
        }
        let tables = store.internal().tables_handle();
        let cache: SharedCache = Arc::default();
        let load = load_func(store, tables.clone(), cache.clone())?;
        let modules = store.internal().generated_modules();
        let module = compile(store, &modules)?;
        let mut imports = Imports::default();
        imports.define("host", "load", load);
        let instance = instantiate(store.internal().runtime_mut(), &module, &imports)
            .await
            .map_err(substrate_failure)?;
        let global = |store: &mut StoreContext<'_, T>, name: &str| {
            instance
                .get_export(store.internal().runtime_mut(), name)
                .map_err(substrate_failure)?
                .and_then(RuntimeExtern::into_global)
                .ok_or_else(|| Error::internal("the context module exports no such global"))
        };
        *lock(&cache) = Some(Cache {
            slots: [global(store, "slot0")?, global(store, "slot1")?],
            loaded: global(store, "loaded")?,
            owner: None,
        });
        store
            .internal()
            .runtime_mut()
            .call_hook(move |context, kind| cross(&tables, &cache, context, kind));
        let slots = Self { instance };
        store.internal().install_context_slots(slots.clone());
        Ok(slots)
    }

    /// The `context.get` built-in of `slot`.
    pub fn get<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        slot: usize,
    ) -> Result<RuntimeFunc> {
        self.export(store, &format!("get{slot}"))
    }

    /// The `context.set` built-in of `slot`.
    pub fn set<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        slot: usize,
    ) -> Result<RuntimeFunc> {
        self.export(store, &format!("set{slot}"))
    }

    fn export<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        name: &str,
    ) -> Result<RuntimeFunc> {
        self.instance
            .get_export(store.internal().runtime_mut(), name)
            .map_err(substrate_failure)?
            .and_then(RuntimeExtern::into_func)
            .ok_or_else(|| Error::internal("the context module exports no such built-in"))
    }
}

/// The context module, compiled once for the engine of `store` and kept in
/// `modules` for every later store of the engine.
fn compile<T: 'static>(
    store: &mut StoreContext<'_, T>,
    modules: &GeneratedModules,
) -> Result<RuntimeModule> {
    let bytes = encode();
    let mut modules = modules.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(module) = modules.get(&bytes) {
        return Ok(module.clone());
    }
    let module = RuntimeModule::new(store.internal().runtime().engine(), &bytes)
        .map_err(substrate_failure)?;
    modules.insert(bytes, module.clone());
    Ok(module)
}

/// The context module: the slots and the loaded flag as mutable `i32`
/// globals, with `getN`, `setN`, the globals as exports, and `load` as
/// its one import.
fn encode() -> Vec<u8> {
    let mut types = TypeSection::new();
    types.ty().function([], []);
    types.ty().function([], [ValType::I32]);
    types.ty().function([ValType::I32], []);
    let (load_type, get_type, set_type) = (0, 1, 2);
    let mut imports = ImportSection::new();
    imports.import("host", "load", EntityType::Function(load_type));
    let load = 0;
    let mutable = GlobalType {
        val_type: ValType::I32,
        mutable: true,
        shared: false,
    };
    let mut globals = GlobalSection::new();
    for _ in 0..=SLOTS {
        globals.global(mutable, &ConstExpr::i32_const(0));
    }
    let load_if_needed = |body: &mut Function| {
        body.instruction(&Instruction::GlobalGet(LOADED))
            .instruction(&Instruction::I32Eqz)
            .instruction(&Instruction::If(BlockType::Empty))
            .instruction(&Instruction::Call(load))
            .instruction(&Instruction::End);
    };
    let mut functions = FunctionSection::new();
    let mut code = CodeSection::new();
    let mut exports = ExportSection::new();
    for slot in 0..SLOTS as u32 {
        let mut get = Function::new([]);
        load_if_needed(&mut get);
        get.instruction(&Instruction::GlobalGet(slot))
            .instruction(&Instruction::End);
        functions.function(get_type);
        code.function(&get);
        exports.export(&format!("get{slot}"), ExportKind::Func, 1 + 2 * slot);

        let mut set = Function::new([]);
        load_if_needed(&mut set);
        set.instruction(&Instruction::LocalGet(0))
            .instruction(&Instruction::GlobalSet(slot))
            .instruction(&Instruction::End);
        functions.function(set_type);
        code.function(&set);
        exports.export(&format!("set{slot}"), ExportKind::Func, 2 + 2 * slot);

        exports.export(&format!("slot{slot}"), ExportKind::Global, slot);
    }
    exports.export("loaded", ExportKind::Global, LOADED);
    let mut module = Encoded::new();
    module
        .section(&types)
        .section(&imports)
        .section(&functions)
        .section(&globals)
        .section(&exports)
        .section(&code);
    module.finish()
}

/// The context module's `load`: copy the current thread's slots into the
/// globals and set the loaded flag.
///
/// It is a plain runtime-layer function, not one of the polyfill's host
/// functions, because no host function of the component runs. The store's
/// call hook runs around it as around every host function. Nothing is
/// loaded when `load` is called, and when it returns, its owner is the
/// current thread, so the slots stay loaded.
fn load_func<T: 'static>(
    store: &mut StoreContext<'_, T>,
    tables: Arc<Mutex<HandleTables>>,
    cache: SharedCache,
) -> Result<RuntimeFunc> {
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        FuncType::new([], []),
        move |mut caller: RuntimeCaller<'_, StoreData<T>>, _params, _results| {
            let (thread, values) = current_slots(&tables)?;
            let mut cache = lock(&cache);
            let cache = cache
                .as_mut()
                .ok_or_else(|| Error::internal("the context module ran before it was installed"))?;
            let mut context = caller.as_context_mut();
            for (global, value) in cache.slots.iter().zip(values) {
                global.set(&mut context, RuntimeVal::I32(value))?;
            }
            cache.loaded.set(&mut context, RuntimeVal::I32(1))?;
            cache.owner = Some(thread);
            Ok(())
        },
    )
    .map_err(substrate_failure)
}

/// The store's call hook at the crossing `kind`: copy the loaded slots to
/// their owner's record when the guest calls the host, and write them
/// back and unload them wherever another thread could run next.
fn cross<T: 'static>(
    tables: &Mutex<HandleTables>,
    cache: &SharedCache,
    mut context: RuntimeContextMut<'_, StoreData<T>>,
    kind: CallHook,
) -> core::result::Result<(), RuntimeError> {
    let mut cache = lock(cache);
    let Some(cache) = cache.as_mut() else {
        return Ok(());
    };
    let Some(owner) = cache.owner else {
        return Ok(());
    };
    match kind {
        CallHook::CallingHost => {
            let values = values(cache, &mut context)?;
            write_slots(tables, owner, values).map_err(backend)
        }
        CallHook::ReturningFromHost if current_thread(tables).map_err(backend)? == Some(owner) => {
            Ok(())
        }
        _ => unload(tables, cache, context, owner),
    }
}

/// Write the loaded slots back to the record of `owner`, the thread they
/// were loaded for, and clear the loaded flag.
///
/// The owner goes only once the flag is clear, so a failure of the
/// substrate leaves the slots loaded for their owner, not for no one. A
/// thread that ended since its slots were loaded has no record, and its
/// slots end with it.
fn unload<T: 'static>(
    tables: &Mutex<HandleTables>,
    cache: &mut Cache,
    mut context: RuntimeContextMut<'_, StoreData<T>>,
    owner: ThreadId,
) -> core::result::Result<(), RuntimeError> {
    let values = values(cache, &mut context)?;
    cache.loaded.set(&mut context, RuntimeVal::I32(0))?;
    cache.owner = None;
    write_slots(tables, owner, values).map_err(backend)
}

/// What the slot globals of `cache` hold.
fn values<T: 'static>(
    cache: &Cache,
    context: &mut RuntimeContextMut<'_, StoreData<T>>,
) -> core::result::Result<[i32; SLOTS], RuntimeError> {
    let mut values = [0; SLOTS];
    for (value, global) in values.iter_mut().zip(&cache.slots) {
        *value = match global.get(&mut *context)? {
            RuntimeVal::I32(value) => value,
            other => {
                return Err(RuntimeError::Backend {
                    message: format!("a context slot's global holds {other:?}, not an i32"),
                });
            }
        };
    }
    Ok(values)
}

/// `error` as an error of the runtime layer, which the call hook answers.
fn backend(error: impl core::fmt::Display) -> RuntimeError {
    RuntimeError::Backend {
        message: error.to_string(),
    }
}

/// The thread that runs now, if any.
fn current_thread(tables: &Mutex<HandleTables>) -> Result<Option<ThreadId>> {
    let tables = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    Ok(tables.tasks.current_thread())
}

/// The current thread and its slots: what `load` copies in.
fn current_slots(tables: &Mutex<HandleTables>) -> anyhow::Result<(ThreadId, [i32; SLOTS])> {
    let tables = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    let thread = tables.tasks.current_thread().ok_or_else(|| {
        anyhow!("an adapter read or wrote a context slot with no task on the stack")
    })?;
    let record = tables
        .tasks
        .thread(thread)
        .ok_or_else(|| Error::internal("the current thread has no record"))?;
    Ok((thread, record.context))
}

/// Write `values` back as the slots of `thread`, if it still has a
/// record.
fn write_slots(tables: &Mutex<HandleTables>, thread: ThreadId, values: [i32; SLOTS]) -> Result<()> {
    let mut tables = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    if let Some(record) = tables.tasks.thread_mut(thread) {
        record.context = values;
    }
    Ok(())
}

fn lock(cache: &SharedCache) -> MutexGuard<'_, Option<Cache>> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_loads_and_writes_back_the_slots_of_the_current_thread() {
        let tables = Mutex::new(HandleTables::new());
        let instance = tables.lock().unwrap().tasks.insert_instance();
        assert!(
            current_slots(&tables).is_err(),
            "there is no thread to load with no task on the stack"
        );

        tables
            .lock()
            .unwrap()
            .tasks
            .push_task(None, None, instance)
            .expect("room under the record cap");
        let (caller, slots) = current_slots(&tables).expect("the caller's thread");
        assert_eq!(slots, [0, 0], "a thread starts with zero slots");
        write_slots(&tables, caller, [7, 8]).expect("the caller's slots go back");

        tables
            .lock()
            .unwrap()
            .tasks
            .push_task(None, None, instance)
            .expect("room under the record cap");
        let (callee, slots) = current_slots(&tables).expect("the callee's thread");
        assert_ne!(callee, caller, "the callee's task brings its own thread");
        assert_eq!(slots, [0, 0], "and its own empty slots");
        write_slots(&tables, callee, [9, 10]).expect("the callee's slots go back");

        tables.lock().unwrap().tasks.pop_scope();
        assert_eq!(
            current_slots(&tables).expect("the caller's thread again"),
            (caller, [7, 8]),
            "the caller finds what it wrote, not what the callee wrote"
        );
    }
}
