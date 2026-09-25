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

/// The provider that fills the suspend capability with the
/// instructions of the WebAssembly stack-switching proposal:
/// `cont.new`, `resume`, and `suspend`.
///
/// It is one instance of a [`SwitchModule`] in the stack-switching
/// form, in the store whose threads it runs. A start calls the
/// module's start for the entry's type, which makes a continuation
/// of the entry wrapper and resumes it. A resume calls the module's
/// resume, which takes the thread's continuation out of the module's
/// table and resumes it. Both run synchronously and return when the
/// thread suspends in a shim or finishes, and both answer from the
/// status the module returns, with the results the entry wrapper
/// handed the host when the thread finished. A suspended thread waits
/// in the module's table of continuations, in its own slot, so any
/// number wait at once and resume in any order. The table lives in
/// the store, so a store that drops drops its suspended threads
/// without resuming them, and no destructor runs.
///
/// The provider works on any engine that implements the proposal.
/// Wasmtime 49 implements it on x86_64 Linux. No browser ships it.
///
/// The provider holds handles to the module's functions and nothing
/// that borrows the store, so it can be cloned out of the store and
/// called with the store beside it.
#[derive(Clone)]
pub struct StackSwitchingProvider {
    starts: Vec<(FuncType, RuntimeFunc)>,
    resume: RuntimeFunc,
    shims: Vec<RuntimeFunc>,
    finished: Finished,
}

impl StackSwitchingProvider {
    /// Compile `module` against `engine` and instantiate it in
    /// `store`.
    ///
    /// `hosts` holds the try and the finish host functions of each
    /// shim of `module`, in the order of the shims. The provider
    /// makes the `finished` host function of each entry type itself.
    /// It fails when `module` is not of the stack-switching form, when
    /// the engine refuses the module, which is what an engine that
    /// does not implement the stack-switching proposal does, or when
    /// `hosts` does not match the shims.
    pub fn instantiate<T: 'static>(
        store: &mut StoreContext<'_, T>,
        engine: &RuntimeEngine<Backend>,
        module: &SwitchModule,
        hosts: &[(RuntimeFunc, RuntimeFunc)],
    ) -> Result<Self> {
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
        let compiled = RuntimeModule::new(engine, &module.encode()).map_err(substrate_failure)?;
        let finished: Finished = Arc::default();
        let mut imports = Imports::default();
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
            let slot = finished.clone();
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
        let instance = RuntimeInstance::new(store.internal().runtime_mut(), &compiled, &imports)
            .map_err(substrate_failure)?;
        let mut export = |name: &str| {
            instance
                .get_export(store.internal().runtime(), name)
                .and_then(RuntimeExtern::into_func)
                .ok_or_else(|| Error::internal("a switch module lacks one of its exports"))
        };
        let starts = module
            .entry_types()
            .iter()
            .enumerate()
            .map(|(j, ty)| Ok((ty.clone(), export(&format!("start{j}"))?)))
            .collect::<Result<Vec<_>>>()?;
        let shims = (0..module.shim_count())
            .map(|i| export(&format!("shim{i}")))
            .collect::<Result<Vec<_>>>()?;
        let resume = export("resume")?;
        Ok(Self {
            starts,
            resume,
            shims,
            finished,
        })
    }

    /// The shim a guest imports in place of the host trampoline of
    /// blocking built-in `index`.
    pub fn shim(&self, index: u32) -> Option<&RuntimeFunc> {
        self.shims.get(index as usize)
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
        let (_, start) = self
            .starts
            .iter()
            .find(|(wrapped, _)| *wrapped == ty)
            .ok_or_else(|| {
                Error::internal("the switch module has no wrapper for the entry's type")
            })?;
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
