//! Baseline tests for the switch module in its JSPI form, next to
//! real core instances, in the browser.
//!
//! The JSPI provider starts each thread entry through
//! `WebAssembly.promising` over the switch module's start, and a
//! thread suspends in a shim of the switch module, through an import
//! made with `WebAssembly.Suspending`, when the blocking built-in it
//! stands for is not ready. The tests run the provider with two guest
//! core instances around it: an entry of the first calls a function
//! of the second, which calls the shim, so a frame of another
//! instance lies between the thread's entry and its suspension. A
//! plain host import that runs inside the first thread starts a
//! second thread through the provider, as a trampoline starts a
//! nested start: the promising call begins a second stack, the
//! second thread suspends, the start returns to the host function,
//! and the host function returns to the first thread, which then
//! suspends too. The browser keeps both stacks, and the tests resume
//! them in either order. A resumption runs on a microtask, so a
//! resume is awaited.
//!
//! The flake's Chromium ships JSPI, and no native engine offers it,
//! so the tests run in the web lane alone.

#![cfg(target_arch = "wasm32")]

use std::collections::HashSet;
use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use js_sys::Promise;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wcmp_macros::wasm;

use crate::concurrency::{EntryStatus, JspiProvider, SuspendProvider, ThreadId};
use crate::internal::EngineInternal;
use crate::runtime_layer::{
    Backend, BackendExtern, BackendFunc, Extern as RuntimeExtern, Func as RuntimeFunc, FuncType,
    Imports, Instance as RuntimeInstance, Module as RuntimeModule, Val as RuntimeVal,
    ValType as RuntimeValType,
};
use crate::store::{StoreContext, StoreContextInternalExt, StoreInternalExt};
use crate::{Engine, Store};

/// The second guest instance. `middle` calls the blocking built-in
/// through the shim it imports and adds one to what it returns, so
/// its frame lies between the shim and whichever entry called it.
const MIDDLE: &[u8] = wasm!(
    r#"
    (module
      (import "switch" "block" (func $block (param i32) (result i32)))
      (func (export "middle") (param i32) (result i32)
        (i32.add (call $block (local.get 0)) (i32.const 1))))
    "#
);

/// The first guest instance, whose exports are the thread entries.
/// `first` calls the host's `spawn` with its key plus one, then
/// calls `middle` with its own key. `second` calls `middle` alone.
const ENTRIES: &[u8] = wasm!(
    r#"
    (module
      (import "other" "middle" (func $middle (param i32) (result i32)))
      (import "host" "spawn" (func $spawn (param i32)))
      (func (export "first") (param i32) (result i32)
        (call $spawn (i32.add (local.get 0) (i32.const 1)))
        (call $middle (local.get 0)))
      (func (export "second") (param i32) (result i32)
        (call $middle (local.get 0))))
    "#
);

/// The first thread, which the test starts from the host.
const FIRST: ThreadId = ThreadId::new(0, 0);

/// The second thread, which `spawn` starts from inside the first.
const SECOND: ThreadId = ThreadId::new(5, 0);

/// The keys whose blocking built-in is ready. The built-in stands in
/// for one that waits on an event: its try part answers whether the
/// key is here, and its finish part answers ten times the key.
type Ready = Arc<Mutex<HashSet<i32>>>;

/// What the second thread's start answered inside `spawn`.
type Spawned = Arc<Mutex<Option<String>>>;

/// The provider, the store it runs in, and the two entries, set up
/// as the scenario of the module's doc states.
struct Scenario {
    store: Store<()>,
    provider: JspiProvider,
    first: RuntimeFunc,
    ready: Ready,
    tries: Arc<Mutex<u32>>,
    spawned: Spawned,
}

fn i32_to_i32() -> FuncType {
    FuncType::new([RuntimeValType::I32], [RuntimeValType::I32])
}

fn describe(status: &EntryStatus) -> String {
    match status {
        EntryStatus::Suspended => "suspended".to_owned(),
        EntryStatus::Finished(results) => format!("finished with {results:?}"),
        EntryStatus::Running => "running".to_owned(),
    }
}

fn setup() -> Scenario {
    let engine = Engine::new().expect("engine");
    let mut store = Store::new(&engine, ()).expect("store");
    let mut context = store.internal().context();
    let ready: Ready = Arc::default();
    let tries: Arc<Mutex<u32>> = Arc::default();
    let spawned: Spawned = Arc::default();

    // The try and the finish of the blocking built-in the shim
    // stands for.
    let tried = ready.clone();
    let counted = tries.clone();
    let try_part = RuntimeFunc::new(
        context.internal().runtime_mut(),
        i32_to_i32(),
        move |_store, args, results| {
            let [RuntimeVal::I32(key)] = args else {
                anyhow::bail!("the try part takes one key");
            };
            *counted.lock().expect("tries") += 1;
            let here = tried.lock().expect("ready keys").contains(key);
            results[0] = RuntimeVal::I32(i32::from(here));
            Ok(())
        },
    );
    let finish_part = RuntimeFunc::new(
        context.internal().runtime_mut(),
        i32_to_i32(),
        move |_store, args, results| {
            let [RuntimeVal::I32(key)] = args else {
                anyhow::bail!("the finish part takes one key");
            };
            results[0] = RuntimeVal::I32(key * 10);
            Ok(())
        },
    );

    let provider = JspiProvider::instantiate(&mut context, engine.switch_modules())
        .expect("the browser offers `WebAssembly.Suspending`");
    let block = provider
        .shims(&mut context, &[(i32_to_i32(), try_part, finish_part)])
        .expect("the browser instantiates the switch module")
        .remove(0);

    let mut imports = Imports::default();
    imports.define("switch", "block", RuntimeExtern::Func(block));
    let middle = RuntimeInstance::new(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), MIDDLE).expect("the module compiles"),
        &imports,
    )
    .expect("the middle instance");

    // `spawn` is a plain import, not a suspending one. It starts the
    // second thread as a nested start, from a host frame that runs
    // inside the first thread, and returns once the start returns.
    let second: Arc<Mutex<Option<RuntimeFunc>>> = Arc::default();
    let spawn = {
        let provider = provider.clone();
        let second = second.clone();
        let spawned = spawned.clone();
        RuntimeFunc::new(
            context.internal().runtime_mut(),
            FuncType::new([RuntimeValType::I32], []),
            move |runtime, args, _results| {
                let entry = second
                    .lock()
                    .expect("the second entry")
                    .clone()
                    .expect("the second entry is set before any thread runs");
                let mut context = StoreContext::new(runtime);
                let status = provider.start(&mut context, SECOND, &entry, &i32_to_i32(), args)?;
                *spawned.lock().expect("record") = Some(describe(&status));
                Ok(())
            },
        )
    };

    let mut imports = Imports::default();
    imports.define(
        "other",
        "middle",
        middle
            .get_export(context.internal().runtime(), "middle")
            .expect("middle"),
    );
    imports.define("host", "spawn", RuntimeExtern::Func(spawn));
    let entries = RuntimeInstance::new(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), ENTRIES).expect("the module compiles"),
        &imports,
    )
    .expect("the entries instance");
    let mut entry = |name: &str| {
        entries
            .get_export(context.internal().runtime(), name)
            .and_then(RuntimeExtern::into_func)
            .expect("an entry export")
    };
    *second.lock().expect("the second entry") = Some(entry("second"));
    let first = entry("first");

    Scenario {
        store,
        provider,
        first,
        ready,
        tries,
        spawned,
    }
}

impl Scenario {
    fn start_first(&mut self, key: i32) -> String {
        let mut context = self.store.internal().context();
        let status = self
            .provider
            .start(
                &mut context,
                FIRST,
                &self.first,
                &i32_to_i32(),
                &[RuntimeVal::I32(key)],
            )
            .expect("the first thread starts");
        describe(&status)
    }

    async fn resume(&mut self, thread: ThreadId) -> String {
        let mut context = self.store.internal().context();
        let status = self
            .provider
            .resume_and_wait(&mut context, thread)
            .await
            .expect("the thread resumes");
        describe(&status)
    }

    fn make_ready(&self, key: i32) {
        self.ready.lock().expect("ready keys").insert(key);
    }

    fn tries(&self) -> u32 {
        *self.tries.lock().expect("tries")
    }

    fn spawned(&self) -> Option<String> {
        self.spawned.lock().expect("record").clone()
    }
}

/// Start the first thread with key 1, which spawns the second with
/// key 2, and check that both suspended.
fn start_both(scenario: &mut Scenario) {
    assert_eq!(
        scenario.start_first(1),
        "suspended",
        "the first thread suspended in the shim, with a frame of the \
         middle instance between its entry and the shim"
    );
    assert_eq!(
        scenario.spawned().as_deref(),
        Some("suspended"),
        "the second thread, started from a plain import inside the first, \
         suspended on a promising stack of its own, and its start returned \
         to the import, which returned to the first thread before that \
         thread suspended"
    );
}

#[wcmp_macros::test]
async fn it_resumes_the_nested_thread_before_the_one_that_started_it() {
    let mut scenario = setup();
    start_both(&mut scenario);

    assert_eq!(
        scenario.resume(SECOND).await,
        "suspended",
        "a thread whose built-in is still not ready suspends again"
    );
    scenario.make_ready(2);
    assert_eq!(
        scenario.resume(SECOND).await,
        "finished with [I32(21)]",
        "the second thread's shim found the built-in ready and returned \
         what its finish computed, through the middle instance"
    );
    scenario.make_ready(1);
    assert_eq!(scenario.resume(FIRST).await, "finished with [I32(11)]");
}

#[wcmp_macros::test]
async fn it_resumes_the_thread_that_started_another_before_the_nested_one() {
    let mut scenario = setup();
    start_both(&mut scenario);

    scenario.make_ready(1);
    scenario.make_ready(2);
    assert_eq!(scenario.resume(FIRST).await, "finished with [I32(11)]");
    assert_eq!(
        scenario.resume(SECOND).await,
        "finished with [I32(21)]",
        "the nested thread outlived the thread and the host frame that \
         started it"
    );
}

#[wcmp_macros::test]
async fn it_returns_from_a_shim_whose_built_in_is_ready_without_a_suspension() {
    // A call of a suspending import always suspends in Chromium, so a
    // shim that called it here would leave the thread suspended, and
    // the start would answer so. The shim calls it only when the
    // built-in is not ready.
    let mut scenario = setup();
    scenario.make_ready(1);
    scenario.make_ready(2);

    assert_eq!(
        scenario.start_first(1),
        "finished with [I32(11)]",
        "the promising call ran the thread to its end without returning \
         to the event loop"
    );
    assert_eq!(
        scenario.spawned().as_deref(),
        Some("finished with [I32(21)]"),
        "the nested thread's shim found its built-in ready too"
    );
    assert_eq!(
        scenario.tries(),
        2,
        "each shim tried its built-in once and never retried after a resume"
    );

    let mut context = scenario.store.internal().context();
    assert!(
        scenario
            .provider
            .resume_and_wait(&mut context, FIRST)
            .await
            .is_err(),
        "the first thread finished, so it holds no suspended promise"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resume_while_another_resume_is_under_way() {
    // A resumed thread runs on a microtask, and the provider knows
    // which thread runs by the order its resumes were made in. A
    // second resume made before the first thread stopped is refused,
    // and leaves its thread suspended for a later resume.
    let mut scenario = setup();
    start_both(&mut scenario);
    scenario.make_ready(1);
    scenario.make_ready(2);

    let mut context = scenario.store.internal().context();
    let resumed = SuspendProvider::resume(&scenario.provider, &mut context, FIRST)
        .expect("the first thread resumes");
    assert_eq!(describe(&resumed), "running");
    assert!(
        SuspendProvider::resume(&scenario.provider, &mut context, SECOND).is_err(),
        "a resume made while another is under way is refused"
    );
    let first = poll_fn(|poll| {
        SuspendProvider::poll_stop(&scenario.provider, &mut context, FIRST, poll.waker())
    })
    .await
    .expect("the first thread stops");
    assert_eq!(describe(&first), "finished with [I32(11)]");
    assert_eq!(
        scenario.resume(SECOND).await,
        "finished with [I32(21)]",
        "the refused resume left the second thread suspended"
    );
}

#[wcmp_macros::test]
async fn it_drops_a_store_with_suspended_threads() {
    let mut scenario = setup();
    start_both(&mut scenario);

    // Nothing resolves either thread's promise: the store and the
    // provider drop with both threads suspended, and the browser's
    // stacks go with the promises.
    drop(scenario);

    let mut again = setup();
    again.make_ready(1);
    again.make_ready(2);
    assert_eq!(again.start_first(1), "finished with [I32(11)]");
}

/// A guest whose `run` calls a suspending import and then a plain
/// one.
const PAUSES: &[u8] = wasm!(
    r#"
    (module
      (import "host" "pause" (func $pause))
      (import "host" "after" (func $after))
      (func (export "run")
        (call $pause)
        (call $after)))
    "#
);

#[wcmp_macros::test]
async fn it_suspends_on_a_suspending_import_whose_promise_is_already_resolved() {
    // The fact the shim rests on. The proposal's overview says a
    // stack suspends only when the import's function answers a
    // promise that is still pending. The browser suspends it anyway,
    // and resumes it on a microtask.
    let engine = Engine::new().expect("engine");
    let mut store = Store::new(&engine, ()).expect("store");
    let mut context = store.internal().context();
    let after = Arc::new(AtomicBool::new(false));

    let pause = BackendFunc::new_suspending(
        context.internal().runtime_mut(),
        FuncType::new([], []),
        |_store, _args| Ok(Promise::resolve(&JsValue::UNDEFINED)),
    )
    .expect("the browser offers `WebAssembly.Suspending`");
    let reached = after.clone();
    let after_import = RuntimeFunc::new(
        context.internal().runtime_mut(),
        FuncType::new([], []),
        move |_store, _args, _results| {
            reached.store(true, Ordering::SeqCst);
            Ok(())
        },
    );
    let mut imports = Imports::default();
    imports.define(
        "host",
        "pause",
        RuntimeExtern::from(&BackendExtern::<Backend>::Func(pause)),
    );
    imports.define("host", "after", RuntimeExtern::Func(after_import));
    let instance = RuntimeInstance::new(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), PAUSES).expect("the module compiles"),
        &imports,
    )
    .expect("the instance");
    let run = match BackendExtern::<Backend>::from(
        &instance
            .get_export(context.internal().runtime(), "run")
            .expect("run"),
    ) {
        BackendExtern::Func(run) => run,
        _ => panic!("`run` is a function"),
    };

    let promise = run
        .call_promising(context.internal().runtime_mut(), &[])
        .expect("the promising call starts");
    assert!(
        !after.load(Ordering::SeqCst),
        "the promising call returned at the suspending import, before the \
         import after it"
    );
    JsFuture::from(promise)
        .await
        .expect("the stack resumed and returned");
    assert!(after.load(Ordering::SeqCst));
}
