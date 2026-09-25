//! The provider that switches stacks with the instructions of the
//! WebAssembly stack-switching proposal.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use wasm_runtime_layer::{
    Engine as RuntimeEngine, Extern as RuntimeExtern, FuncType, Imports,
    Instance as RuntimeInstance, Module as RuntimeModule, ValType as RuntimeValType,
};
use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

use crate::backend::{Backend, substrate_failure};
use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::store::{StoreContext, StoreContextInternalExt};

use super::entry_status::EntryStatus;
use super::suspend_provider::SuspendProvider;
use super::switch_form::SwitchForm;
use super::switch_module::SwitchModule;
use super::thread_id::ThreadId;

/// The results each thread's entry wrapper handed the host when the
/// entry returned, by thread index, until a start or resume takes
/// them.
type Finished = Arc<Mutex<HashMap<u32, Vec<RuntimeVal>>>>;

/// The compiled switch modules of one engine, by their bytes.
type Compiled = Arc<Mutex<HashMap<Vec<u8>, RuntimeModule>>>;

/// The provider that fills the suspend capability with the
/// instructions of the WebAssembly stack-switching proposal:
/// `cont.new`, `resume`, and `suspend`.
///
/// It is one instance of the [`SwitchModule`]'s base module in the
/// store whose threads it runs, and the extension modules the store
/// has needed so far. A start calls the start of the extension module
/// for the entry's type, which hands the entry to the base module's
/// start, and that runs the entry wrapper on a worker. A resume calls
/// the base module's resume, which takes the thread's continuation
/// out of the table and resumes it. Both run synchronously and return
/// when the thread suspends in a shim or finishes, and both answer
/// from the status the module returns, with the results the entry
/// wrapper handed the host when the thread finished. A suspended
/// thread waits in the base module's table of continuations, in its
/// own slot, so any number wait at once and resume in any order. The
/// table lives in the store, so a store that drops drops its
/// suspended threads without resuming them, and no destructor runs.
///
/// The provider makes the extension module for an entry type the
/// first time a thread of that type starts, and the extension module
/// with the shims of an instantiation when that instantiation asks
/// for them. The engine compiles each distinct module once, and every
/// store of the engine instantiates the compiled module.
///
/// The provider keys a thread's slot by the index of its record, as
/// the store's table of threads does. The generation of a
/// [`ThreadId`] is the scheduler's to check: it resumes only a thread
/// it parked, so a thread whose record index a later thread took is
/// never resumed in the later thread's place.
///
/// The provider works on any engine that implements the proposal.
/// Wasmtime 49 implements it on x86_64 Linux. No browser ships it.
///
/// The provider holds handles to the modules' functions and nothing
/// that borrows the store. The store keeps it for its whole life, and
/// a caller clones the handle out and calls it with the store beside
/// it.
#[derive(Clone)]
pub struct StackSwitchingProvider {
    engine: RuntimeEngine<Backend>,
    compiled: Compiled,
    start: RuntimeFunc,
    resume: RuntimeFunc,
    suspend: RuntimeFunc,
    workers: RuntimeFunc,
    starts: Arc<Mutex<Vec<(FuncType, RuntimeFunc)>>>,
    finished: Finished,
}

impl StackSwitchingProvider {
    /// Compile the base module against `engine`, or take it from
    /// `compiled`, and instantiate it in `store`.
    ///
    /// It fails when the engine refuses the module, which is what an
    /// engine that does not implement the stack-switching proposal
    /// does.
    pub fn instantiate<T: 'static>(
        store: &mut StoreContext<'_, T>,
        engine: &RuntimeEngine<Backend>,
        compiled: &Arc<Mutex<HashMap<Vec<u8>, RuntimeModule>>>,
    ) -> Result<Self> {
        let module = compile(engine, compiled, SwitchModule::base())?;
        let instance =
            RuntimeInstance::new(store.internal().runtime_mut(), &module, &Imports::default())
                .map_err(substrate_failure)?;
        let mut export = |name: &str| export_func(store, &instance, name);
        Ok(Self {
            engine: engine.clone(),
            compiled: compiled.clone(),
            start: export("start")?,
            resume: export("resume")?,
            suspend: export("suspend")?,
            workers: export("workers")?,
            starts: Arc::default(),
            finished: Arc::default(),
        })
    }

    /// Make the shims of the blocking built-ins in `hosts`, one for
    /// each, in the order given. Each entry names the shim's type, the
    /// built-in's try, and its finish.
    pub fn shims<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        hosts: &[(FuncType, RuntimeFunc, RuntimeFunc)],
    ) -> Result<Vec<RuntimeFunc>> {
        if hosts.is_empty() {
            return Ok(Vec::new());
        }
        let mut module = SwitchModule::new(SwitchForm::StackSwitching);
        for (ty, _, _) in hosts {
            module.shim(ty.clone());
        }
        let parts = hosts
            .iter()
            .map(|(_, try_part, finish_part)| (try_part.clone(), finish_part.clone()))
            .collect::<Vec<_>>();
        let instance = self.extension(store, &module, &parts)?;
        (0..module.shim_count())
            .map(|i| export_func(store, &instance, &format!("shim{i}")))
            .collect()
    }

    /// Instantiate the extension module `module` describes, with the
    /// try and finish of each of its shims in `hosts` and a
    /// `finished` recorder for each of its entry types.
    fn extension<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        module: &SwitchModule,
        hosts: &[(RuntimeFunc, RuntimeFunc)],
    ) -> Result<RuntimeInstance> {
        if module.form() != SwitchForm::StackSwitching {
            return Err(Error::internal(
                "the stack-switching provider needs the stack-switching form of the switch module",
            ));
        }
        if hosts.len() != module.shim_count() as usize {
            return Err(Error::internal(
                "a switch module needs a try and a finish for each shim",
            ));
        }
        let compiled = compile(&self.engine, &self.compiled, module.encode())?;
        let mut imports = Imports::default();
        imports.define("base", "start", RuntimeExtern::Func(self.start.clone()));
        imports.define("base", "suspend", RuntimeExtern::Func(self.suspend.clone()));
        for (i, (try_part, finish_part)) in hosts.iter().enumerate() {
            imports.define(
                "host",
                &format!("try{i}"),
                RuntimeExtern::Func(try_part.clone()),
            );
            imports.define(
                "host",
                &format!("finish{i}"),
                RuntimeExtern::Func(finish_part.clone()),
            );
        }
        for (j, ty) in module.entry_types().iter().enumerate() {
            let slot = self.finished.clone();
            let recorder = RuntimeFunc::new(
                store.internal().runtime_mut(),
                FuncType::new(
                    [RuntimeValType::I32]
                        .into_iter()
                        .chain(ty.results().iter().copied()),
                    [],
                ),
                move |_store, args, _results| {
                    let Some((RuntimeVal::I32(thread), results)) = args.split_first() else {
                        return Err(anyhow::anyhow!(
                            "an entry wrapper finished without its thread index"
                        ));
                    };
                    slot.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(thread.cast_unsigned(), results.to_vec());
                    Ok(())
                },
            );
            imports.define(
                "host",
                &format!("finished{j}"),
                RuntimeExtern::Func(recorder),
            );
        }
        RuntimeInstance::new(store.internal().runtime_mut(), &compiled, &imports)
            .map_err(substrate_failure)
    }

    /// The start for entries of type `ty`, made the first time a
    /// thread of that type starts.
    fn start_for<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        ty: &FuncType,
    ) -> Result<RuntimeFunc> {
        let known = self
            .starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(wrapped, _)| wrapped == ty)
            .map(|(_, start)| start.clone());
        if let Some(start) = known {
            return Ok(start);
        }
        let mut module = SwitchModule::new(SwitchForm::StackSwitching);
        module.entry(ty.clone());
        let instance = self.extension(store, &module, &[])?;
        let start = export_func(store, &instance, "start0")?;
        self.starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((ty.clone(), start.clone()));
        Ok(start)
    }

    /// How many workers the store's switch module has made: the
    /// continuations it holds a stack for, which is as many as the
    /// store ever had threads alive at once, and not one for every
    /// thread it started.
    pub fn workers<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> Result<u32> {
        let mut made = [RuntimeVal::I32(0)];
        self.workers
            .call(store.internal().runtime_mut(), &[], &mut made)
            .map_err(substrate_failure)?;
        match made {
            [RuntimeVal::I32(made)] => Ok(made.cast_unsigned()),
            _ => Err(Error::internal("a switch module counted its workers wrong")),
        }
    }

    /// Answer the status a start or a resume of `thread` returned.
    fn status(&self, thread: u32, status: &RuntimeVal) -> Result<EntryStatus> {
        match status {
            RuntimeVal::I32(SwitchModule::SUSPENDED) => Ok(EntryStatus::Suspended),
            RuntimeVal::I32(SwitchModule::FINISHED) => self
                .finished
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&thread)
                .map(EntryStatus::Finished)
                .ok_or_else(|| Error::internal("a thread finished without handing its results")),
            _ => Err(Error::internal(
                "a switch module answered an unknown status",
            )),
        }
    }
}

impl<T: 'static> SuspendProvider<T> for StackSwitchingProvider {
    fn start(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        let ty = entry.ty(store.internal().runtime());
        let start = self.start_for(store, &ty)?;
        let index = thread.index();
        let arguments = [
            RuntimeVal::I32(index.cast_signed()),
            RuntimeVal::FuncRef(Some(entry.clone())),
        ]
        .into_iter()
        .chain(args.iter().cloned())
        .collect::<Vec<_>>();
        let mut status = [RuntimeVal::I32(0)];
        start
            .call(store.internal().runtime_mut(), &arguments, &mut status)
            .map_err(substrate_failure)?;
        self.status(index, &status[0])
    }

    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus> {
        let index = thread.index();
        let mut status = [RuntimeVal::I32(0)];
        self.resume
            .call(
                store.internal().runtime_mut(),
                &[RuntimeVal::I32(index.cast_signed())],
                &mut status,
            )
            .map_err(substrate_failure)?;
        self.status(index, &status[0])
    }
}

/// The module `bytes` encode, compiled against `engine` once and kept
/// in `compiled` for every later store of the engine.
fn compile(
    engine: &RuntimeEngine<Backend>,
    compiled: &Compiled,
    bytes: Vec<u8>,
) -> Result<RuntimeModule> {
    let mut cache = compiled.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(module) = cache.get(&bytes) {
        return Ok(module.clone());
    }
    let module = RuntimeModule::new(engine, &bytes).map_err(substrate_failure)?;
    cache.insert(bytes, module.clone());
    Ok(module)
}

/// The function `instance` exports as `name`.
fn export_func<T: 'static>(
    store: &mut StoreContext<'_, T>,
    instance: &RuntimeInstance,
    name: &str,
) -> Result<RuntimeFunc> {
    instance
        .get_export(store.internal().runtime(), name)
        .and_then(RuntimeExtern::into_func)
        .ok_or_else(|| Error::internal("a switch module lacks one of its exports"))
}
