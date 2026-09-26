//! The provider that switches stacks through JavaScript Promise
//! Integration.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Poll, Waker};

use js_sys::{Function, Promise, Reflect};
use js_wasm_runtime_layer::Func as BackendFunc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_runtime_layer::backend::{Extern as BackendExtern, Val as BackendVal};
use wasm_runtime_layer::{
    Engine as RuntimeEngine, Extern as RuntimeExtern, Func as RuntimeFunc, FuncType, Imports,
    Instance as RuntimeInstance, Module as RuntimeModule, Val as RuntimeVal,
    ValType as RuntimeValType,
};

use crate::backend::{Backend, substrate_failure};
use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::store::{StoreContext, StoreContextInternalExt};

use super::entry_status::EntryStatus;
use super::suspend_provider::SuspendProvider;
use super::switch_form::SwitchForm;
use super::switch_module::SwitchModule;
use super::thread_id::ThreadId;

/// The compiled switch modules of one engine, by their bytes.
type Compiled = Arc<Mutex<HashMap<Vec<u8>, RuntimeModule>>>;

/// The provider that fills the suspend capability through JavaScript
/// Promise Integration (JSPI), in a browser that ships it.
///
/// It is a set of instances of the [`SwitchModule`] in the JSPI form,
/// in the store whose threads it runs: one with the shims of each
/// instantiation, made when the instantiation asks for them, and one
/// with the start for each type of thread entry, made the first time
/// a thread of that type starts. The engine compiles each distinct
/// module once, and every store of the engine instantiates the
/// compiled module. The form has no base module, because the browser
/// keeps each suspended stack, and every instance shares the one
/// `suspend` import the provider makes with `WebAssembly.Suspending`.
///
/// A start calls the start for the entry's type through
/// `WebAssembly.promising`. The call runs the thread synchronously,
/// on a stack of its own, until the thread finishes or first
/// suspends. A shim suspends by calling `suspend`. The function
/// behind that import answers a promise and keeps the promise's
/// resolver, and the browser keeps the suspended stack until the
/// promise resolves. A resume resolves the promise. The browser then
/// resumes the stack on a microtask, never inside the call that
/// resolved it, so a resume answers [`EntryStatus::Running`], and
/// [`poll_stop`](SuspendProvider::poll_stop) answers once the thread
/// suspends again or finishes. The resumed shim tries its built-in
/// again and returns the result its finish computes, as in the
/// stack-switching form, so the promise resolves with no value.
///
/// A promising call answers a promise and never the entry's results.
/// The provider learns at once that an entry finished from the
/// module's entry wrapper, which hands the results to the host
/// before it returns. A shim calls `suspend` only when its built-in
/// is not ready, because Chromium 147 suspends on every call of a
/// suspending import, even when its function answers a resolved
/// promise.
///
/// A promising call from a host function that runs inside one thread
/// begins a stack of its own for a second thread. The provider keeps
/// a stack of the threads that run, innermost last, each with the
/// number of calls from the host into the guest that were running
/// when its stack began. The thread at its top is the one whose shim
/// calls `suspend`, and a thread leaves it when it suspends,
/// finishes, or fails. Any number of threads wait at once, each on
/// its own promise, and they resume in any order. A resume is made
/// only while no thread runs and no other resume is under way, so the
/// thread a resume pushes onto the stack is the one that runs next.
///
/// A thread that traps or throws rejects its promise, which the
/// browser reports on a microtask. The store turns the reason into
/// the error a call would report, the host's own error first. A start
/// whose thread failed before it first suspended fails at once with
/// the host's own error when a host function failed. Otherwise it
/// answers [`EntryStatus::Running`], and the trap itself comes from
/// `poll_stop` once the browser reports it, so the failure carries
/// the trap's own message.
///
/// A store that drops drops the switch modules with it, and nothing
/// resolves a suspended thread's promise, so the thread is never
/// resumed and no destructor runs. The provider holds the resolvers,
/// and dropping its last clone drops them. The handler that watches
/// the promise of such a thread is never called and stays allocated.
/// A thread whose promise a resume already resolved runs all the same,
/// on its microtask. A store that drops before that thread stops is
/// therefore kept, and marked dropped. The resumed shim's try finds
/// the mark, answers that the store dropped, and has the store freed
/// on a later microtask, and the shim traps. The thread's stack
/// unwinds where it suspended, and no guest code, host import, or
/// destructor runs in the dropped store.
///
/// The provider holds handles to the modules' functions and a key to
/// its state, and nothing that borrows the store or that JavaScript
/// owns. It can therefore be cloned out of the store, captured by a
/// host function, and called with the store beside it. The state
/// lives in a table local to the one thread a browser runs the page
/// on.
#[derive(Clone)]
pub struct JspiProvider {
    compiled: Compiled,
    suspend: RuntimeExtern,
    starts: Arc<Mutex<Vec<(FuncType, RuntimeFunc)>>>,
    registration: Arc<Registration>,
}

/// Where a thread stopped since a start or a resume last asked,
/// until the next one takes it.
enum Stop {
    /// The thread suspended in a shim, and its resolver waits in
    /// [`Threads::resolvers`].
    Suspended,
    /// The entry returned these results.
    Finished(Vec<RuntimeVal>),
    /// The thread's promise was rejected with this reason.
    Failed(JsValue),
}

/// One thread that runs on a stack of the provider.
#[derive(Clone, Copy)]
struct Running {
    /// The thread's index.
    thread: u32,
    /// How many calls from the host into the guest were running when
    /// the thread's stack began or resumed.
    depth: usize,
}

/// The state of one provider's threads.
#[derive(Default)]
struct Threads {
    /// The threads that run on a stack of the provider, innermost
    /// last.
    running: Vec<Running>,
    /// Where each thread stopped, by thread index, until a start or a
    /// resume takes it.
    stops: HashMap<u32, Stop>,
    /// The resolver of each suspended thread's promise.
    resolvers: HashMap<u32, Function>,
    /// The waker of the caller that waits for each thread to stop.
    wakers: HashMap<u32, Waker>,
    /// The start of each thread that has neither finished nor failed,
    /// numbered so that the handler of an earlier start of the same
    /// index does not take a later thread's failure for its own.
    live: HashMap<u32, u64>,
    /// The number of the next start.
    next_start: u64,
    /// The thread whose resume is under way: its promise is resolved,
    /// and it has not stopped yet.
    resuming: Option<u32>,
}

impl Threads {
    /// Record that `thread` stopped at `stop`, take it off the stack
    /// of running threads, and answer the waker of the caller that
    /// waits for it, which the caller wakes once it released the
    /// state.
    fn stopped(&mut self, thread: u32, stop: Stop) -> Option<Waker> {
        if self.running.last().map(|running| running.thread) == Some(thread) {
            self.running.pop();
        } else {
            self.running.retain(|running| running.thread != thread);
        }
        if !matches!(stop, Stop::Suspended) {
            self.live.remove(&thread);
        }
        self.stops.insert(thread, stop);
        self.wakers.remove(&thread)
    }
}

thread_local! {
    /// The state of every provider, by the key its registration
    /// holds. A browser runs the page, its WebAssembly, and every
    /// promise callback on one thread, so the state of a provider
    /// is always here, and it may hold JavaScript values.
    static THREADS: RefCell<HashMap<u64, Threads>> = RefCell::new(HashMap::new());
}

/// The key of the next provider.
static NEXT_KEY: AtomicU64 = AtomicU64::new(0);

/// A provider's key to its state, which drops the state when the
/// last clone of the provider drops.
struct Registration(u64);

impl Registration {
    fn new() -> Self {
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        THREADS.with_borrow_mut(|all| all.insert(key, Threads::default()));
        Self(key)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        THREADS.with_borrow_mut(|all| all.remove(&self.0));
    }
}

/// Run `f` over the state of the provider `key`. It answers `None`
/// when the provider is gone. `f` must not call anything that can
/// reach a provider's state again, such as a guest.
fn with_threads<R>(key: u64, f: impl FnOnce(&mut Threads) -> R) -> Option<R> {
    THREADS.with_borrow_mut(|all| all.get_mut(&key).map(f))
}

/// The error of a host function of the switch module that runs after
/// its provider dropped.
fn provider_gone() -> anyhow::Error {
    anyhow::anyhow!("the switch module outlived its JSPI provider")
}

/// The error of a provider whose state is gone.
fn state_lost() -> Error {
    Error::internal("the JSPI provider lost its state")
}

impl JspiProvider {
    /// Make the provider's `suspend` import in `store`, with the
    /// modules the store's engine compiled so far in `compiled`.
    ///
    /// It fails when the browser has no `WebAssembly.Suspending`.
    pub fn instantiate<T: 'static>(
        store: &mut StoreContext<'_, T>,
        compiled: &Compiled,
    ) -> Result<Self> {
        let registration = Arc::new(Registration::new());
        let key = registration.0;
        let suspend = BackendFunc::new_suspending(
            store.internal().runtime_mut(),
            FuncType::new([], []),
            move |_store, _args| suspend(key),
        )
        .map_err(substrate_failure)?;
        Ok(Self {
            compiled: compiled.clone(),
            suspend: runtime_func(suspend),
            starts: Arc::default(),
            registration,
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
        let mut module = SwitchModule::new(SwitchForm::Jspi);
        for (ty, _, _) in hosts {
            module.shim(ty.clone());
        }
        let parts = hosts
            .iter()
            .map(|(_, try_part, finish_part)| (try_part.clone(), finish_part.clone()))
            .collect::<Vec<_>>();
        let instance = self.module(store, &module, &parts)?;
        (0..module.shim_count())
            .map(|i| export_func(store, &instance, &format!("shim{i}")))
            .collect()
    }

    /// Instantiate the module `module` describes, with the try and
    /// finish of each of its shims in `hosts`, a `finished` recorder
    /// for each of its entry types, and the provider's `suspend`.
    fn module<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        module: &SwitchModule,
        hosts: &[(RuntimeFunc, RuntimeFunc)],
    ) -> Result<RuntimeInstance> {
        if module.form() != SwitchForm::Jspi {
            return Err(Error::internal(
                "the JSPI provider needs the JSPI form of the switch module",
            ));
        }
        if hosts.len() != module.shim_count() as usize {
            return Err(Error::internal(
                "a switch module needs a try and a finish for each shim",
            ));
        }
        let key = self.registration.0;
        let engine = store.internal().runtime().engine().clone();
        let compiled = compile(&engine, &self.compiled, module.encode())?;
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
                    let stop = Stop::Finished(results.to_vec());
                    let waker =
                        with_threads(key, |threads| threads.stopped(thread.cast_unsigned(), stop))
                            .ok_or_else(provider_gone)?;
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                    Ok(())
                },
            );
            imports.define(
                "host",
                &format!("finished{j}"),
                RuntimeExtern::Func(recorder),
            );
        }
        imports.define("host", "suspend", self.suspend.clone());
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
        let mut module = SwitchModule::new(SwitchForm::Jspi);
        module.entry(ty.clone());
        let instance = self.module(store, &module, &[])?;
        let start = export_func(store, &instance, "start0")?;
        self.starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((ty.clone(), start.clone()));
        Ok(start)
    }

    /// Whether a shim called from the host function that runs now may
    /// suspend the stack it runs on: a thread of the provider runs,
    /// and no call from the host into the guest was made since its
    /// stack began or resumed, so only WebAssembly frames lie between
    /// the start of that stack and the shim.
    pub fn may_suspend<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        let depth = guest_depth(store);
        with_threads(self.registration.0, |threads| {
            threads
                .running
                .last()
                .is_some_and(|running| running.depth == depth)
        })
        .unwrap_or(false)
    }

    /// Whether the store runs no guest code now: no thread of the
    /// provider runs and no call from the host into the guest is under
    /// way. Only then may a resume be made, since the resumed thread
    /// runs on a microtask that nothing below it waits for.
    pub fn at_rest<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        guest_depth(store) == 0
            && with_threads(self.registration.0, |threads| threads.running.is_empty())
                .unwrap_or(false)
    }

    /// Keep `store` allocated when it drops while a resumed thread has
    /// yet to run, and mark it dropped. That thread runs on a
    /// microtask and reaches the store as it runs: the try of the shim
    /// it suspended in finds the mark, frees the store with
    /// [`release_dropped`](Self::release_dropped), and answers that
    /// the store dropped, and the shim traps. A thread that already
    /// stopped reaches the store no more, so the store drops at once
    /// then.
    pub fn retain_if_resuming<T: 'static>(&self, store: &mut StoreContext<'_, T>) {
        let resuming = with_threads(self.registration.0, |threads| {
            threads
                .resuming
                .is_some_and(|thread| !threads.stops.contains_key(&thread))
        })
        .unwrap_or(false);
        if resuming {
            store.internal().mark_dropped();
            store.internal().runtime_mut().inner.retain_on_drop();
        }
    }

    /// Free `store`, which its owner dropped while a resumed thread
    /// had yet to run, on a microtask. The thread calls this from its
    /// shim's try, and traps as the try returns, so the store is freed
    /// once nothing reaches it.
    pub fn release_dropped<T: 'static>(&self, store: &mut StoreContext<'_, T>) {
        store.internal().runtime_mut().inner.release_orphaned();
    }

    /// Resume the suspended `thread`, and complete when it finishes
    /// or suspends again, with what [`poll_stop`](SuspendProvider::poll_stop)
    /// answers then.
    pub async fn resume_and_wait<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
    ) -> Result<EntryStatus> {
        match SuspendProvider::resume(self, store, thread)? {
            EntryStatus::Running => {}
            stopped => return Ok(stopped),
        }
        poll_fn(|context| SuspendProvider::poll_stop(self, store, thread, context.waker())).await
    }
}

impl<T: 'static> SuspendProvider<T> for JspiProvider {
    fn start(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        ty: &FuncType,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        let start = self.start_for(store, ty)?;
        let key = self.registration.0;
        let index = thread.index();
        let depth = guest_depth(store);
        let number = with_threads(key, |threads| {
            threads.stops.remove(&index);
            threads.resolvers.remove(&index);
            threads.running.push(Running {
                thread: index,
                depth,
            });
            let number = threads.next_start;
            threads.next_start += 1;
            threads.live.insert(index, number);
            number
        })
        .ok_or_else(state_lost)?;
        let arguments = [
            RuntimeVal::I32(index.cast_signed()),
            RuntimeVal::FuncRef(Some(entry.clone())),
        ]
        .iter()
        .chain(args)
        .map(BackendVal::<Backend>::from)
        .collect::<Vec<_>>();
        let called = backend_func(&start).and_then(|start| {
            start
                .call_promising(store.internal().runtime_mut(), &arguments)
                .map_err(substrate_failure)
        });
        let promise = match called {
            Ok(promise) => promise,
            Err(error) => {
                forget(key, index);
                return Err(error);
            }
        };
        watch(key, index, number, &promise);
        match with_threads(key, |threads| threads.stops.remove(&index)).flatten() {
            Some(stop) => answer(store, stop),
            None => {
                // The promising call returned, and the thread neither
                // finished nor suspended, so it failed, and the
                // browser rejects its promise on a microtask. A host
                // function's failure is the reason whatever the
                // promise carries, so it is reported now. A trap of
                // the guest's own comes with the rejection.
                let backend = &mut store.internal().runtime_mut().inner;
                if backend.pending_failure() {
                    forget(key, index);
                    let failure = backend.failure(&JsValue::UNDEFINED);
                    return Err(substrate_failure(failure));
                }
                with_threads(key, |threads| {
                    threads.running.retain(|running| running.thread != index);
                });
                Ok(EntryStatus::Running)
            }
        }
    }

    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus> {
        let key = self.registration.0;
        let index = thread.index();
        let depth = guest_depth(store);
        let resolver = with_threads(key, |threads| {
            if threads.resuming.is_some() || !threads.running.is_empty() {
                return Err(Error::internal(
                    "a JSPI thread was resumed while another thread runs",
                ));
            }
            let resolver = threads
                .resolvers
                .remove(&index)
                .ok_or_else(|| Error::internal("cannot resume a thread which is not suspended"))?;
            threads.running.push(Running {
                thread: index,
                depth,
            });
            threads.resuming = Some(index);
            Ok(resolver)
        })
        .ok_or_else(state_lost)??;
        if let Err(reason) = resolver.call0(&JsValue::UNDEFINED) {
            forget(key, index);
            with_threads(key, |threads| threads.resuming = None);
            return Err(substrate_failure(
                store.internal().runtime_mut().inner.failure(&reason),
            ));
        }
        Ok(EntryStatus::Running)
    }

    fn poll_stop(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        waker: &Waker,
    ) -> Poll<Result<EntryStatus>> {
        let index = thread.index();
        let stop = with_threads(self.registration.0, |threads| {
            let stop = threads.stops.remove(&index);
            match stop {
                Some(_) => {
                    if threads.resuming == Some(index) {
                        threads.resuming = None;
                    }
                }
                None => {
                    threads.wakers.insert(index, waker.clone());
                }
            }
            stop
        });
        match stop {
            Some(Some(stop)) => Poll::Ready(answer(store, stop)),
            Some(None) => Poll::Pending,
            None => Poll::Ready(Err(state_lost())),
        }
    }
}

/// How many calls from the host into the guest are running in
/// `store`.
fn guest_depth<T: 'static>(store: &mut StoreContext<'_, T>) -> usize {
    store.internal().runtime_mut().inner.guest_depth()
}

/// The function behind the `suspend` import of the provider `key`:
/// suspend the thread at the top of the stack of running threads on
/// a new promise, and keep the promise's resolver for the resume.
fn suspend(key: u64) -> anyhow::Result<Promise> {
    let mut resolver = None;
    let promise = Promise::new(&mut |resolve, _reject| resolver = Some(resolve));
    let resolver = resolver.ok_or_else(|| anyhow::anyhow!("a promise ran no executor"))?;
    let waker = with_threads(key, |threads| {
        let thread = threads.running.last()?.thread;
        threads.resolvers.insert(thread, resolver);
        Some(threads.stopped(thread, Stop::Suspended))
    })
    .ok_or_else(provider_gone)?
    .ok_or_else(|| anyhow::anyhow!("a shim suspended outside every thread of its provider"))?;
    if let Some(waker) = waker {
        waker.wake();
    }
    Ok(promise)
}

/// Watch the promise of the start numbered `number` of `thread`. When
/// it is rejected while that start's thread still lives, the thread
/// failed, and the reason is where it stopped. A promise that
/// resolves settles after its thread finished, and the handler finds
/// the thread gone.
fn watch(key: u64, thread: u32, number: u64, promise: &Promise) {
    let settled = Closure::once_into_js(move |reason: JsValue| {
        let waker = with_threads(key, |threads| {
            if threads.live.get(&thread) != Some(&number) {
                return None;
            }
            threads.stopped(thread, Stop::Failed(reason))
        })
        .flatten();
        if let Some(waker) = waker {
            waker.wake();
        }
    });
    // `then` with one function for both outcomes: the function runs
    // once, and frees itself when it does.
    let then = Reflect::get(promise, &"then".into())
        .ok()
        .and_then(|then| then.dyn_into::<Function>().ok());
    if let Some(then) = then {
        let _ = then.call2(promise, &settled, &settled);
    }
}

/// Forget `thread` after a start or a resume that failed before the
/// thread ran, or whose failure the start reported itself.
fn forget(key: u64, thread: u32) {
    with_threads(key, |threads| {
        threads.running.retain(|running| running.thread != thread);
        threads.live.remove(&thread);
    });
}

/// What a start or a resume answers for the thread that stopped at
/// `stop`. A thread that suspended or finished leaves no host error
/// behind for the next failure to report: a guest that caught one
/// went on.
fn answer<T: 'static>(store: &mut StoreContext<'_, T>, stop: Stop) -> Result<EntryStatus> {
    let backend = &mut store.internal().runtime_mut().inner;
    match stop {
        Stop::Suspended => {
            backend.clear_failure();
            Ok(EntryStatus::Suspended)
        }
        Stop::Finished(results) => {
            backend.clear_failure();
            Ok(EntryStatus::Finished(results))
        }
        Stop::Failed(reason) => Err(substrate_failure(backend.failure(&reason))),
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

/// The browser backend's own handle of `func`.
fn backend_func(func: &RuntimeFunc) -> Result<BackendFunc> {
    match BackendExtern::<Backend>::from(&RuntimeExtern::Func(func.clone())) {
        BackendExtern::Func(func) => Ok(func),
        _ => Err(Error::internal("a function converted to another extern")),
    }
}

/// The runtime layer's handle of the browser backend's `func`.
fn runtime_func(func: BackendFunc) -> RuntimeExtern {
    RuntimeExtern::from(&BackendExtern::<Backend>::Func(func))
}
