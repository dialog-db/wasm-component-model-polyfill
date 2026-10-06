// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The provider that switches stacks through the runtime layer's host
//! suspension.

use core::future::{Future, poll_fn};
use core::pin::pin;
use core::task::{Poll, Waker};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::{Error, Result, ThreadCause};
use crate::internal::ErrorInternal;
use crate::runtime_layer::{
    Extern as RuntimeExtern, Func as RuntimeFunc, FuncType, HostFrames, Imports,
    Instance as RuntimeInstance, Module as RuntimeModule, ResumableCall, Resumption, Shared,
    SuspendedCall, TrapKind, Val as RuntimeVal, ValType as RuntimeValType, at_once, call_failure,
    host_func, instantiate, substrate_failure,
};
use crate::store::{StoreContext, StoreContextInternalExt};

use super::entry_status::EntryStatus;
use super::suspend_provider::SuspendProvider;
use super::switch_form::SwitchForm;
use super::switch_module::SwitchModule;
use super::thread_id::ThreadId;

/// The compiled switch modules of one engine, by their bytes.
type Compiled = Arc<Mutex<HashMap<Vec<u8>, RuntimeModule>>>;

/// The provider that fills the suspend capability through the runtime
/// layer's host suspension, on a backend that declares it: the
/// browser's backend through JavaScript Promise Integration, or a
/// backend whose engine suspends a call itself.
///
/// It is a set of instances of the [`SwitchModule`] in the
/// host-suspension form, in the store whose threads it runs: one with
/// the shims of each instantiation, and one with the starts for the
/// types of thread entry each instantiation can start, both made while
/// the instantiation runs. The engine compiles each distinct module
/// once, and every store of the engine instantiates the compiled
/// module. The form has no base module, because the backend keeps each
/// suspended call, and every instance shares the one `suspend` import
/// the provider makes as a suspending host function.
///
/// A start calls the start for the entry's type as a resumable call.
/// The call runs the thread on a stack of its own, until the thread
/// finishes or first suspends. A shim suspends by calling `suspend`,
/// which answers "not yet", and the call ends suspended with a handle
/// the provider keeps. A resume resumes that handle. The resumed shim
/// tries its built-in again and returns the result its finish
/// computes, as in the stack-switching form.
///
/// A resumable call ends through a future, because the browser runs a
/// call's stack on a microtask and delivers its end through a promise.
/// The future borrows the store, and a host function cannot wait for
/// it. So a resume made where the store runs no guest code, in a turn
/// of a driver, becomes the store's flight: the provider answers
/// [`EntryStatus::Running`], the turn ends, and the driver awaits the
/// flight through [`fly`](Self::fly) with nothing else in between.
/// [`poll_stop`](SuspendProvider::poll_stop) then answers where the
/// thread stopped. A start made there runs in place up to the thread's
/// first stop, which a backend sees on the first poll of a call that
/// suspends or finishes in its first stretch. Only a start whose call
/// has not stopped by then, one that trapped in the browser, becomes
/// the store's flight, under way, so that the trap's reason reaches the
/// scheduler whole. A start made from inside a guest call, a
/// nested start, becomes the store's flight too, through
/// [`defer_start`](Self::defer_start), where the thread that makes it
/// can suspend for it and leave the rest of its built-in as a plan.
/// Anywhere else a nested start runs its first stretch on the first
/// poll of the call's future: a call that suspends ends on that poll on
/// every backend.
///
/// A resumable call does not answer the entry's results. The provider
/// learns at once that an entry finished from the module's entry
/// wrapper, which hands the results to the host before it returns. So
/// a nested start in place whose future has not ended on its first poll
/// finished when the wrapper handed over its results, and failed
/// otherwise: its failure reaches only a caller that waits for the
/// future, which a host function cannot, so the provider reports it as a
/// trap of its own, without the engine's reason.
///
/// A thread may suspend only where WebAssembly frames alone lie between
/// the start of its stack and the shim. The provider keeps a stack of
/// the threads that run, innermost last, each with the number of host
/// frames that were running when its stack began or resumed. The shim
/// of the thread at the top may suspend when the one host frame above
/// that number is the built-in's own.
///
/// A store that drops drops its waiting calls with it, and no
/// destructor runs. Dropping a driver cancels nothing: a flight that a
/// driver no longer awaits, because its future dropped, stays with the
/// store as a resumption under way. The backend lets the thread run on
/// as far as it can without the store, where it waits, and the next
/// driver takes the flight up and awaits the thread's stop in its place,
/// as Wasmtime lets the threads of a task whose host future dropped run
/// on for other tasks of the component.
///
/// The provider holds handles to the modules' functions and its
/// state behind a lock, and nothing that borrows the store. It can
/// therefore be cloned out of the store, captured by a host function,
/// and called with the store beside it.
#[derive(Clone)]
pub struct HostSuspensionProvider {
    compiled: Shared<Compiled>,
    suspend: RuntimeFunc,
    starts: Arc<Mutex<Vec<(FuncType, RuntimeFunc)>>>,
    threads: Shared<Arc<Mutex<Threads>>>,
    /// The results each thread's entry wrapper handed the host when
    /// the entry returned, by thread index, until a start or a stop
    /// takes them. They sit apart from the rest of the state because
    /// the host functions that record them must be `Send` on every
    /// target, and a waiting call is not in the browser.
    finished: Finished,
}

/// The results each thread's entry wrapper handed the host.
type Finished = Arc<Mutex<HashMap<u32, Vec<RuntimeVal>>>>;

/// One thread whose stack runs now.
#[derive(Clone, Copy)]
struct Running {
    /// The thread's index.
    thread: u32,
    /// How many host frames were running when the thread's stack
    /// began or resumed.
    frames: usize,
}

/// A start or a resume that a driver awaits.
enum Flight {
    /// The start of `thread`: `start`, called with `arguments`.
    Start {
        thread: u32,
        start: RuntimeFunc,
        arguments: Vec<RuntimeVal>,
    },
    /// The resume of `thread`, whose call waits in `call`.
    Resume { thread: u32, call: SuspendedCall },
    /// The start or the resume of `thread` that runs as `resumption`,
    /// whose driver dropped before the thread stopped. The next driver
    /// takes it up.
    Underway { thread: u32, resumption: Resumption },
}

impl Flight {
    /// The thread the flight runs.
    fn thread(&self) -> u32 {
        match self {
            Self::Start { thread, .. }
            | Self::Resume { thread, .. }
            | Self::Underway { thread, .. } => *thread,
        }
    }
}

/// The state of one provider's threads.
#[derive(Default)]
struct Threads {
    /// The threads whose stacks run now, innermost last.
    running: Vec<Running>,
    /// The call of each suspended thread, by thread index.
    waiting: HashMap<u32, SuspendedCall>,
    /// The start or resume a driver must await next.
    flight: Option<Flight>,
    /// Where each thread a flight ran stopped, by thread index, until
    /// [`poll_stop`](SuspendProvider::poll_stop) takes it.
    stops: HashMap<u32, Result<EntryStatus>>,
}

impl Threads {
    /// Take `thread` off the stack of running threads.
    fn leave(&mut self, thread: u32) {
        if self.running.last().map(|running| running.thread) == Some(thread) {
            self.running.pop();
        } else {
            self.running.retain(|running| running.thread != thread);
        }
    }

    /// Where `thread`, whose call ended with `outcome`, stopped, with
    /// the results its entry wrapper handed over in `finished`.
    fn stopped(
        &mut self,
        finished: &Finished,
        thread: u32,
        outcome: core::result::Result<ResumableCall, crate::runtime_layer::RuntimeError>,
    ) -> Result<EntryStatus> {
        let results = lock(finished).remove(&thread);
        match outcome {
            Ok(ResumableCall::Suspended(call)) => {
                self.waiting.insert(thread, call);
                Ok(EntryStatus::Suspended)
            }
            Ok(ResumableCall::Finished) => finished_with(results),
            Ok(_) => Err(Error::internal(
                "a resumable call ended in a way the provider does not know",
            )),
            Err(error) => Err(call_failure(error)),
        }
    }
}

/// Where a thread whose entry wrapper handed over `results` stopped.
fn finished_with(results: Option<Vec<RuntimeVal>>) -> Result<EntryStatus> {
    results
        .map(EntryStatus::Finished)
        .ok_or_else(|| Error::internal("a thread finished without handing its results"))
}

/// Lock `mutex`, reading past a poison: the state stays consistent
/// between each two steps of the provider.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The error of a host function of the switch module that runs after
/// its provider lost its state.
fn state_lost() -> Error {
    Error::internal("the host-suspension provider lost its state")
}

/// Leaves a flight that runs to the next driver where the driver that
/// awaited it drops it first. The flight's thread runs on as far as it can
/// without the store, and waits there for the next driver, which takes the
/// flight up and awaits the thread's stop in its place.
struct Handoff {
    threads: Arc<Mutex<Threads>>,
    thread: u32,
    resumption: Option<Resumption>,
}

impl Drop for Handoff {
    fn drop(&mut self) {
        let Some(resumption) = self.resumption.take() else {
            return;
        };
        let mut threads = lock(&self.threads);
        threads.leave(self.thread);
        threads.flight = Some(Flight::Underway {
            thread: self.thread,
            resumption,
        });
    }
}

impl HostSuspensionProvider {
    /// Make the provider's `suspend` import in `store`, with the
    /// modules the store's engine compiled so far in `compiled`.
    ///
    /// It fails when the backend does not declare host suspension.
    pub fn instantiate<T: 'static>(
        store: &mut StoreContext<'_, T>,
        compiled: &Compiled,
    ) -> Result<Self> {
        let suspend = RuntimeFunc::new_suspending(
            store.internal().runtime_mut(),
            FuncType::new([], []),
            |_caller, _args, _results| Ok(Poll::Pending),
        )
        .map_err(substrate_failure)?;
        Ok(Self {
            compiled: Shared::new(compiled.clone()),
            suspend,
            starts: Arc::default(),
            threads: Shared::new(Arc::default()),
            finished: Arc::default(),
        })
    }

    /// The state of the provider's threads.
    fn threads(&self) -> MutexGuard<'_, Threads> {
        lock(&self.threads)
    }

    /// Make the shims of the blocking built-ins in `hosts`, one for
    /// each, in the order given. Each entry names the shim's type, the
    /// built-in's try, and its finish.
    pub async fn shims<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        hosts: &[(FuncType, RuntimeFunc, RuntimeFunc)],
    ) -> Result<Vec<RuntimeFunc>> {
        if hosts.is_empty() {
            return Ok(Vec::new());
        }
        let mut module = SwitchModule::new(SwitchForm::HostSuspension);
        for (ty, _, _) in hosts {
            module.shim(ty.clone());
        }
        let parts = hosts
            .iter()
            .map(|(_, try_part, finish_part)| (*try_part, *finish_part))
            .collect::<Vec<_>>();
        let (compiled, imports) = self.module(store, &module, &parts)?;
        let instance = instantiate(store.internal().runtime_mut(), &compiled, &imports)
            .await
            .map_err(substrate_failure)?;
        (0..module.shim_count())
            .map(|i| export_func(store, &instance, &format!("shim{i}")))
            .collect()
    }

    /// Make the starts for the entry types among `types` that have none
    /// yet, so that a thread of each can start while a guest runs, where
    /// nothing can wait for an instantiation.
    pub async fn prepare_entries<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        types: &[FuncType],
    ) -> Result<()> {
        let missing = {
            let starts = self.starts.lock().unwrap_or_else(PoisonError::into_inner);
            let mut missing: Vec<FuncType> = Vec::new();
            for ty in types {
                if !starts.iter().any(|(known, _)| known == ty) && !missing.contains(ty) {
                    missing.push(ty.clone());
                }
            }
            missing
        };
        if missing.is_empty() {
            return Ok(());
        }
        let mut module = SwitchModule::new(SwitchForm::HostSuspension);
        for ty in &missing {
            module.entry(ty.clone());
        }
        let (compiled, imports) = self.module(store, &module, &[])?;
        let instance = instantiate(store.internal().runtime_mut(), &compiled, &imports)
            .await
            .map_err(substrate_failure)?;
        self.keep_starts(store, &instance, &missing)
    }

    /// Keep the start `instance` exports for each of `types`, in order.
    fn keep_starts<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        instance: &RuntimeInstance,
        types: &[FuncType],
    ) -> Result<()> {
        let made = types
            .iter()
            .enumerate()
            .map(|(j, ty)| {
                Ok((
                    ty.clone(),
                    export_func(store, instance, &format!("start{j}"))?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(made);
        Ok(())
    }

    /// The compiled module `module` describes, and its imports: the try
    /// and finish of each of its shims in `hosts`, a `finished`
    /// recorder for each of its entry types, and the provider's
    /// `suspend`.
    fn module<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        module: &SwitchModule,
        hosts: &[(RuntimeFunc, RuntimeFunc)],
    ) -> Result<(RuntimeModule, Imports)> {
        if module.form() != SwitchForm::HostSuspension {
            return Err(Error::internal(
                "the host-suspension provider needs the host-suspension form of the switch module",
            ));
        }
        if hosts.len() != module.shim_count() as usize {
            return Err(Error::internal(
                "a switch module needs a try and a finish for each shim",
            ));
        }
        let engine = store.internal().runtime().engine().clone();
        let compiled = compile(&engine, &self.compiled, module.encode())?;
        let mut imports = Imports::default();
        for (i, (try_part, finish_part)) in hosts.iter().enumerate() {
            imports.define("host", &format!("try{i}"), *try_part);
            imports.define("host", &format!("finish{i}"), *finish_part);
        }
        for (j, ty) in module.entry_types().iter().enumerate() {
            let finished = self.finished.clone();
            let recorder = host_func(
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
                    lock(&finished).insert(thread.cast_unsigned(), results.to_vec());
                    Ok(())
                },
            )?;
            imports.define("host", &format!("finished{j}"), recorder);
        }
        imports.define("host", "suspend", self.suspend);
        Ok((compiled, imports))
    }

    /// The start for entries of type `ty`.
    ///
    /// An instantiation makes the starts its threads need before any of
    /// them runs. A start for another type is made here, where the
    /// backend instantiates at once, and is an error where it does not,
    /// as the browser's backend does not.
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
            .map(|(_, start)| *start);
        if let Some(start) = known {
            return Ok(start);
        }
        let mut module = SwitchModule::new(SwitchForm::HostSuspension);
        module.entry(ty.clone());
        let (compiled, imports) = self.module(store, &module, &[])?;
        let instance = at_once(instantiate(
            store.internal().runtime_mut(),
            &compiled,
            &imports,
        ))
        .ok_or_else(|| Error::internal("a thread entry of a type no instantiation prepared"))?
        .map_err(substrate_failure)?;
        self.keep_starts(store, &instance, core::slice::from_ref(ty))?;
        export_func(store, &instance, "start0")
    }

    /// Whether a shim called from the host function that runs now may
    /// suspend the stack it runs on: a thread of the provider runs, and
    /// the one host frame above the frames that ran when its stack
    /// began or resumed is the host function's own, so only
    /// WebAssembly frames lie between the start of that stack and the
    /// shim.
    pub fn may_suspend<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        let frames = host_frames(store);
        self.threads()
            .running
            .last()
            .is_some_and(|running| running.frames + 1 == frames)
    }

    /// Whether the store runs no guest code now: no thread of the
    /// provider runs, no host function runs, and no flight waits for a
    /// driver. Only then may a start or a resume become the store's
    /// flight.
    pub fn at_rest<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        let threads = self.threads();
        host_frames(store) == 0 && threads.running.is_empty() && threads.flight.is_none()
    }

    /// Start `entry`, whose core type is `ty`, with `args` as `thread`,
    /// as the store's flight, from a frame inside a guest call whose
    /// thread suspends for it: its shim may suspend here, as
    /// [`may_suspend`](Self::may_suspend) answers. The thread runs once
    /// the driver awaits it, so its end reaches the scheduler whole, a
    /// trap's reason included, which a start in place inside a host
    /// function cannot wait for. It answers [`EntryStatus::Running`].
    pub fn defer_start<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        ty: &FuncType,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        if self.threads().flight.is_some() {
            return Err(Error::internal(
                "a thread start was left to the store while another start or resume waits",
            ));
        }
        let start = self.start_for(store, ty)?;
        let index = thread.index();
        let arguments = [
            RuntimeVal::I32(index.cast_signed()),
            RuntimeVal::FuncRef(Some(*entry)),
        ]
        .into_iter()
        .chain(args.iter().copied())
        .collect::<Vec<_>>();
        {
            let mut threads = self.threads();
            threads.stops.remove(&index);
            threads.waiting.remove(&index);
            threads.flight = Some(Flight::Start {
                thread: index,
                start,
                arguments,
            });
        }
        lock(&self.finished).remove(&index);
        Ok(EntryStatus::Running)
    }

    /// Let go of the start of `thread` that
    /// [`defer_start`](Self::defer_start) left as the store's flight,
    /// before any driver ran it, for a store a trap poisoned: the thread
    /// never runs. A flight of another thread, or one already under way,
    /// stays.
    pub fn abandon_start(&self, thread: ThreadId) {
        let mut threads = self.threads();
        if matches!(
            threads.flight,
            Some(Flight::Start { thread: index, .. }) if index == thread.index()
        ) {
            threads.flight = None;
        }
    }

    /// Start `thread` with `start`, called with `arguments`, where the
    /// store runs no guest code: in place, up to the thread's first stop.
    ///
    /// A backend sees a call that suspends or finishes in its first
    /// stretch stop on the first poll of its wait, the browser's included,
    /// so such a start answers where the thread stopped at once, and the
    /// driver's turn goes on. A call that has not stopped there is one
    /// that trapped in the browser, which learns the reason only once the
    /// browser settles the call. It becomes the store's flight, under way,
    /// and the provider answers [`EntryStatus::Running`]: the driver awaits
    /// the thread's stop, so its end reaches the scheduler whole, the
    /// trap's reason included.
    fn start_at_rest<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: u32,
        start: RuntimeFunc,
        arguments: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        self.threads().running.push(Running { thread, frames: 0 });
        let runtime = store.internal().runtime_mut();
        let mut resumption = match start.start_resumable(&mut *runtime, arguments) {
            Ok(resumption) => resumption,
            Err(error) => {
                let mut threads = self.threads();
                threads.leave(thread);
                return threads.stopped(&self.finished, thread, Err(error));
            }
        };
        let mut results: [RuntimeVal; 0] = [];
        let outcome = at_once(resumption.stop(&mut *runtime, &mut results));
        let mut threads = self.threads();
        threads.leave(thread);
        match outcome {
            Some(outcome) => threads.stopped(&self.finished, thread, outcome),
            // The entry wrapper handed over the results of an entry that
            // finished, and the thread reaches the store no more.
            None if lock(&self.finished).contains_key(&thread) => {
                finished_with(lock(&self.finished).remove(&thread))
            }
            None => {
                threads.flight = Some(Flight::Underway { thread, resumption });
                Ok(EntryStatus::Running)
            }
        }
    }

    /// Run the store's flight, the start or the resume a turn left for
    /// the driver, to the thread's next stop, and keep where it
    /// stopped for [`poll_stop`](SuspendProvider::poll_stop). A store
    /// with no flight has nothing to run.
    ///
    /// Where the future drops before the thread stops, the flight stays
    /// with the store, under way: the thread runs on as far as it can
    /// without the store, and the next driver's `fly` takes it up and
    /// runs it to that stop. Dropping a driver cancels nothing.
    pub async fn fly<T: 'static>(&self, store: &mut StoreContext<'_, T>) {
        let Some(flight) = self.threads().flight.take() else {
            return;
        };
        let thread = flight.thread();
        self.threads().running.push(Running { thread, frames: 0 });
        let runtime = store.internal().runtime_mut();
        let resumption = match flight {
            Flight::Start {
                start, arguments, ..
            } => start.start_resumable(&mut *runtime, &arguments),
            Flight::Resume { call, .. } => call.start_resume(&mut *runtime, &[]),
            Flight::Underway { resumption, .. } => Ok(resumption),
        };
        let outcome = match resumption {
            Ok(resumption) => {
                let mut handoff = Handoff {
                    threads: Arc::clone(&self.threads),
                    thread,
                    resumption: Some(resumption),
                };
                let mut results: [RuntimeVal; 0] = [];
                let outcome = match handoff.resumption.as_mut() {
                    Some(resumption) => {
                        let stop = resumption.stop(&mut *runtime, &mut results);
                        until_finished(stop, &self.finished, thread).await
                    }
                    None => None,
                };
                // The thread stopped, or handed over its results and
                // reaches the store no more: nothing is left to take up.
                handoff.resumption = None;
                outcome
            }
            Err(error) => Some(Err(error)),
        };
        let mut threads = self.threads();
        threads.leave(thread);
        let stop = match outcome {
            Some(outcome) => threads.stopped(&self.finished, thread, outcome),
            None => finished_with(lock(&self.finished).remove(&thread)),
        };
        threads.stops.insert(thread, stop);
    }
}

impl<T: 'static> SuspendProvider<T> for HostSuspensionProvider {
    fn start(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        ty: &FuncType,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        let start = self.start_for(store, ty)?;
        let index = thread.index();
        let arguments = [
            RuntimeVal::I32(index.cast_signed()),
            RuntimeVal::FuncRef(Some(*entry)),
        ]
        .into_iter()
        .chain(args.iter().copied())
        .collect::<Vec<_>>();
        {
            let mut threads = self.threads();
            threads.stops.remove(&index);
            threads.waiting.remove(&index);
        }
        lock(&self.finished).remove(&index);
        if self.at_rest(store) {
            return self.start_at_rest(store, index, start, &arguments);
        }
        let frames = host_frames(store);
        self.threads().running.push(Running {
            thread: index,
            frames,
        });
        let mut results: [RuntimeVal; 0] = [];
        let outcome =
            at_once(start.call_resumable(store.internal().runtime_mut(), &arguments, &mut results));
        let mut threads = self.threads();
        threads.leave(index);
        match outcome {
            Some(outcome) => threads.stopped(&self.finished, index, outcome),
            // The call's future ends only once the backend settles it,
            // which a host function cannot wait for. The entry wrapper
            // handed over the results of an entry that finished.
            None => match lock(&self.finished).remove(&index) {
                Some(results) => finished_with(Some(results)),
                None => Err(call_failure(crate::runtime_layer::RuntimeError::Trap(
                    TrapKind::Other(
                        "a thread started from inside a guest call trapped before it first \
                     suspended"
                            .to_owned(),
                    ),
                ))),
            },
        }
    }

    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus> {
        let index = thread.index();
        if !self.at_rest(store) {
            return Err(Error::internal(
                "a thread was resumed while the store runs guest code",
            ));
        }
        let mut threads = self.threads();
        let call = threads
            .waiting
            .remove(&index)
            .ok_or(Error::Thread(ThreadCause::NotSuspended))?;
        threads.stops.remove(&index);
        threads.flight = Some(Flight::Resume {
            thread: index,
            call,
        });
        Ok(EntryStatus::Running)
    }

    fn poll_stop(
        &self,
        _store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        _waker: &Waker,
    ) -> Poll<Result<EntryStatus>> {
        let mut threads = self.threads();
        match threads.stops.remove(&thread.index()) {
            Some(stop) => Poll::Ready(stop),
            // The driver awaits the flight once the turn has ended, and
            // takes up the stop in its next turn.
            None if threads
                .flight
                .as_ref()
                .is_some_and(|flight| flight.thread() == thread.index()) =>
            {
                Poll::Pending
            }
            None => Poll::Ready(Err(state_lost())),
        }
    }
}

/// How many host functions of the polyfill run in `store` now.
fn host_frames<T: 'static>(store: &mut StoreContext<'_, T>) -> usize {
    *store.internal().runtime_mut().data_mut().host_frames()
}

/// The module `bytes` encode, compiled against `engine` once and kept
/// in `compiled` for every later store of the engine.
fn compile(
    engine: &crate::runtime_layer::Engine,
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
        .get_export(store.internal().runtime_mut(), name)
        .map_err(substrate_failure)?
        .and_then(RuntimeExtern::into_func)
        .ok_or_else(|| Error::internal("a switch module lacks one of its exports"))
}

/// Await `call`, the resumable call that runs `thread`, until it ends
/// or until the thread's entry wrapper handed its results to
/// `finished`, which answers `None`.
///
/// The thread reaches the store no more once its wrapper handed over
/// its results, and the wrapper only returns after that. The call
/// itself ends once the backend settles it, which in the browser is
/// on a microtask, and the driver does not wait for that end. The
/// runtime layer keeps the store for such a call until it ends, and
/// keeps the call from the store should it reach the store again.
async fn until_finished<F: Future>(call: F, finished: &Finished, thread: u32) -> Option<F::Output> {
    let mut call = pin!(call);
    poll_fn(|context| match call.as_mut().poll(context) {
        Poll::Ready(outcome) => Poll::Ready(Some(outcome)),
        Poll::Pending if lock(finished).contains_key(&thread) => Poll::Ready(None),
        Poll::Pending => Poll::Pending,
    })
    .await
}
