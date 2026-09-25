//! The provider that switches stacks through JavaScript Promise
//! Integration.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
use super::switch_form::SwitchForm;
use super::switch_module::SwitchModule;
use super::thread_id::ThreadId;

/// The provider that fills the suspend capability through JavaScript
/// Promise Integration (JSPI), in a browser that ships it.
///
/// It is one instance of a [`SwitchModule`] in the JSPI form, in the
/// store whose threads it runs. A start calls the module's start for
/// the entry's type through `WebAssembly.promising`. The call runs
/// the thread synchronously, on a stack of its own, until the thread
/// finishes or first suspends. A shim suspends by calling the
/// module's `suspend` import, which the provider makes with
/// `WebAssembly.Suspending`. The function behind that import answers
/// a promise and keeps the promise's resolver, and the browser keeps
/// the suspended stack until the promise resolves. A resume resolves
/// the promise. The browser then resumes the stack on a microtask,
/// never inside the call that resolved it, so a resume is a future:
/// it completes when the thread suspends again or finishes. The
/// resumed shim tries its built-in again and returns the result its
/// finish computes, as in the stack-switching form, so the promise
/// resolves with no value.
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
/// a stack of the threads that run, innermost last. The thread at
/// its top is the one whose shim calls `suspend`, and a thread leaves
/// it when it suspends, finishes, or fails. Any number of threads
/// wait at once, each on its own promise, and they resume in any
/// order.
///
/// A thread that traps or throws rejects its promise, which the
/// browser reports on a microtask. A resume reads the reason, and
/// the store turns it into the error a call would report, the host's
/// own error first. A start cannot wait for the microtask, so a
/// thread that fails before it first suspends makes the start fail
/// with the host's own error when a host function failed, and
/// otherwise with a message that says the entry failed before it
/// suspended.
///
/// A store that drops drops the switch module with it, and nothing
/// resolves a suspended thread's promise, so the thread is never
/// resumed and no destructor runs. The provider holds the resolvers,
/// and dropping its last clone drops them. The handler that watches
/// the promise of such a thread is never called and stays allocated.
///
/// The provider holds handles to the module's functions and a key to
/// its state, and nothing that borrows the store or that JavaScript
/// owns. It can therefore be cloned out of the store, captured by a
/// host function, and called with the store beside it. The state
/// lives in a table local to the one thread a browser runs the page
/// on.
///
/// A start answers synchronously, as the scheduler's contract for a
/// provider states. A resume cannot, so the provider has an
/// asynchronous resume of its own.
#[derive(Clone)]
pub struct JspiProvider {
    starts: Vec<(FuncType, RuntimeFunc)>,
    shims: Vec<RuntimeFunc>,
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

/// The state of one provider's threads.
#[derive(Default)]
struct Threads {
    /// The threads that run on a stack of the provider, innermost
    /// last.
    running: Vec<u32>,
    /// Where each thread stopped, by thread index, until a start or a
    /// resume takes it.
    stops: HashMap<u32, Stop>,
    /// The resolver of each suspended thread's promise.
    resolvers: HashMap<u32, Function>,
    /// The waker of the resume that waits for each thread.
    wakers: HashMap<u32, Waker>,
    /// The start of each thread that has neither finished nor failed,
    /// numbered so that the handler of an earlier start of the same
    /// index does not take a later thread's failure for its own.
    live: HashMap<u32, u64>,
    /// The number of the next start.
    next_start: u64,
}

impl Threads {
    /// Record that `thread` stopped at `stop`, take it off the stack
    /// of running threads, and answer the waker of the resume that
    /// waits for it, which the caller wakes once it released the
    /// state.
    fn stopped(&mut self, thread: u32, stop: Stop) -> Option<Waker> {
        if self.running.last() == Some(&thread) {
            self.running.pop();
        } else {
            self.running.retain(|running| *running != thread);
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

impl JspiProvider {
    /// Compile `module` against `engine` and instantiate it in
    /// `store`.
    ///
    /// `hosts` holds the try and the finish host functions of each
    /// shim of `module`, in the order of the shims. The provider
    /// makes the `finished` host function of each entry type and the
    /// `suspend` import itself. It fails when `module` is not of the
    /// JSPI form, when `hosts` does not match the shims, and when the
    /// browser has no `WebAssembly.Suspending`.
    pub fn instantiate<T: 'static>(
        store: &mut StoreContext<'_, T>,
        engine: &RuntimeEngine<Backend>,
        module: &SwitchModule,
        hosts: &[(RuntimeFunc, RuntimeFunc)],
    ) -> Result<Self> {
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
        let registration = Arc::new(Registration::new());
        let key = registration.0;
        let compiled = RuntimeModule::new(engine, &module.encode()).map_err(substrate_failure)?;
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
        let suspend = BackendFunc::new_suspending(
            store.internal().runtime_mut(),
            FuncType::new([], []),
            move |_store, _args| suspend(key),
        )
        .map_err(substrate_failure)?;
        imports.define("host", "suspend", runtime_func(suspend));
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
        Ok(Self {
            starts,
            shims,
            registration,
        })
    }

    /// The shim a guest imports in place of the host trampoline of
    /// blocking built-in `index`.
    pub fn shim(&self, index: u32) -> Option<&RuntimeFunc> {
        self.shims.get(index as usize)
    }

    /// Start `entry` with `args` as `thread`, on a stack of its own,
    /// and run it until it finishes or first suspends.
    ///
    /// The caller is the scheduler or a trampoline. From a
    /// trampoline this is a nested start: the new thread runs above
    /// the trampoline's frame, and control comes back to the
    /// trampoline when the thread suspends or finishes.
    ///
    /// It answers the entry's results when the entry finished, and
    /// [`EntryStatus::Suspended`] when it suspended, in which case
    /// the provider keeps the thread until a [`resume`](Self::resume)
    /// names it. It fails when the entry fails before it first
    /// suspends, or when the provider has no wrapper for the entry's
    /// type.
    pub fn start<T: 'static>(
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
        let key = self.registration.0;
        let index = thread.index();
        let number = with_threads(key, |threads| {
            threads.stops.remove(&index);
            threads.resolvers.remove(&index);
            threads.running.push(index);
            let number = threads.next_start;
            threads.next_start += 1;
            threads.live.insert(index, number);
            number
        })
        .ok_or_else(|| Error::internal("the JSPI provider lost its state"))?;
        let arguments = [
            RuntimeVal::I32(index.cast_signed()),
            RuntimeVal::FuncRef(Some(entry.clone())),
        ]
        .iter()
        .chain(args)
        .map(BackendVal::<Backend>::from)
        .collect::<Vec<_>>();
        let called = backend_func(start).and_then(|start| {
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
                // finished nor suspended, so it failed. The browser
                // rejects its promise on a microtask, which a start
                // cannot wait for.
                forget(key, index);
                Err(substrate_failure(
                    store.internal().runtime_mut().inner.failure(
                        &js_sys::Error::new("the thread's entry failed before it first suspended")
                            .into(),
                    ),
                ))
            }
        }
    }

    /// Resume the suspended `thread`, and complete when it finishes
    /// or suspends again. It answers as [`start`](Self::start) does,
    /// and fails when `thread` is not suspended or when it fails
    /// after the resume.
    ///
    /// The resumption runs on a microtask, never inside this call, so
    /// the store must stay beside the provider until the future
    /// completes, and the caller runs nothing else in between.
    pub async fn resume<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
    ) -> Result<EntryStatus> {
        let key = self.registration.0;
        let index = thread.index();
        let resolver = with_threads(key, |threads| {
            let resolver = threads.resolvers.remove(&index)?;
            threads.running.push(index);
            Some(resolver)
        })
        .flatten()
        .ok_or_else(|| Error::internal("cannot resume a thread which is not suspended"))?;
        if let Err(reason) = resolver.call0(&JsValue::UNDEFINED) {
            forget(key, index);
            return Err(substrate_failure(
                store.internal().runtime_mut().inner.failure(&reason),
            ));
        }
        let stop = poll_fn(|context| {
            let stop = with_threads(key, |threads| {
                let stop = threads.stops.remove(&index);
                if stop.is_none() {
                    threads.wakers.insert(index, context.waker().clone());
                }
                stop
            });
            match stop {
                Some(Some(stop)) => Poll::Ready(Some(stop)),
                Some(None) => Poll::Pending,
                None => Poll::Ready(None),
            }
        })
        .await
        .ok_or_else(|| Error::internal("the JSPI provider lost its state"))?;
        answer(store, stop)
    }
}

/// The function behind the `suspend` import of the provider `key`:
/// suspend the thread at the top of the stack of running threads on
/// a new promise, and keep the promise's resolver for the resume.
fn suspend(key: u64) -> anyhow::Result<Promise> {
    let mut resolver = None;
    let promise = Promise::new(&mut |resolve, _reject| resolver = Some(resolve));
    let resolver = resolver.ok_or_else(|| anyhow::anyhow!("a promise ran no executor"))?;
    let waker = with_threads(key, |threads| {
        let thread = *threads.running.last()?;
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
/// thread ran.
fn forget(key: u64, thread: u32) {
    with_threads(key, |threads| {
        threads.running.retain(|running| *running != thread);
        threads.live.remove(&thread);
    });
}

/// What a start or a resume answers for the thread that stopped at
/// `stop`.
fn answer<T: 'static>(store: &mut StoreContext<'_, T>, stop: Stop) -> Result<EntryStatus> {
    match stop {
        Stop::Suspended => Ok(EntryStatus::Suspended),
        Stop::Finished(results) => Ok(EntryStatus::Finished(results)),
        Stop::Failed(reason) => Err(substrate_failure(
            store.internal().runtime_mut().inner.failure(&reason),
        )),
    }
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
