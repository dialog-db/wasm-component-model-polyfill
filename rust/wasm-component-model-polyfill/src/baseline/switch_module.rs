//! Baseline tests for the switch module in its stack-switching form,
//! next to real core instances.
//!
//! The stack-switching provider runs each thread entry as a
//! continuation of the switch module's entry wrapper, and a thread
//! suspends in a shim of the switch module when the blocking
//! built-in it stands for is not ready. The tests run the provider
//! with two guest core instances around it: an entry of the first
//! calls a function of the second, which calls the shim, so a frame
//! of another instance lies between the thread's entry and its
//! suspension. A host function that runs inside the first thread
//! starts a second thread through the provider, as a trampoline
//! starts a nested start: the second thread suspends, the start
//! returns to the host function, and the host function returns to
//! the first thread, which then suspends too. Both threads wait in
//! the switch module's table, and the tests resume them in either
//! order.
//!
//! Wasmtime 49 implements the stack-switching proposal on x86_64
//! Linux only, and no browser ships it, so the tests run there alone.

#![cfg(all(target_arch = "x86_64", target_os = "linux"))]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{
    Extern as RuntimeExtern, Func as RuntimeFunc, FuncType, Imports, Instance as RuntimeInstance,
    Module as RuntimeModule, Val as RuntimeVal, ValType as RuntimeValType,
};
use wcmp_macros::wasm;

use crate::concurrency::{
    EntryStatus, StackSwitchingProvider, SuspendProvider, SwitchForm, SwitchModule, ThreadId,
};
use crate::internal::EngineInternal;
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
    provider: StackSwitchingProvider,
    first: RuntimeFunc,
    ready: Ready,
    spawned: Spawned,
}

fn i32_to_i32() -> FuncType {
    FuncType::new([RuntimeValType::I32], [RuntimeValType::I32])
}

fn describe(status: &EntryStatus) -> String {
    match status {
        EntryStatus::Suspended => "suspended".to_owned(),
        EntryStatus::Finished(results) => format!("finished with {results:?}"),
    }
}

fn setup() -> Scenario {
    let engine = Engine::new().expect("engine");
    let mut store = Store::new(&engine, ()).expect("store");
    let mut context = store.internal().context();
    let ready: Ready = Arc::default();
    let spawned: Spawned = Arc::default();

    // The try and the finish of the blocking built-in the shim
    // stands for.
    let tried = ready.clone();
    let try_part = RuntimeFunc::new(
        context.internal().runtime_mut(),
        i32_to_i32(),
        move |_store, args, results| {
            let [RuntimeVal::I32(key)] = args else {
                anyhow::bail!("the try part takes one key");
            };
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

    let provider =
        StackSwitchingProvider::instantiate(&mut context, engine.inner(), engine.switch_modules())
            .expect("the engine instantiates the base switch module");
    let shim = provider
        .shims(&mut context, &[(i32_to_i32(), try_part, finish_part)])
        .expect("the engine instantiates the extension with the shim")
        .remove(0);

    let mut imports = Imports::default();
    imports.define("switch", "block", RuntimeExtern::Func(shim));
    let middle = RuntimeInstance::new(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), MIDDLE).expect("the module compiles"),
        &imports,
    )
    .expect("the middle instance");

    // `spawn` starts the second thread as a nested start, from a
    // host frame that runs inside the first thread, and returns once
    // the start returns.
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
                let status = provider.start(&mut context, SECOND, &entry, args)?;
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
        spawned,
    }
}

impl Scenario {
    fn start_first(&mut self, key: i32) -> String {
        let mut context = self.store.internal().context();
        let status = self
            .provider
            .start(&mut context, FIRST, &self.first, &[RuntimeVal::I32(key)])
            .expect("the first thread starts");
        describe(&status)
    }

    fn resume(&mut self, thread: ThreadId) -> String {
        let mut context = self.store.internal().context();
        let status = self
            .provider
            .resume(&mut context, thread)
            .expect("the thread resumes");
        describe(&status)
    }

    fn make_ready(&self, key: i32) {
        self.ready.lock().expect("ready keys").insert(key);
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
        "the second thread, started from the host function inside the first, \
         suspended, and its start returned to the host function, which \
         returned to the first thread before that thread suspended"
    );
}

#[wcmp_macros::test]
fn it_resumes_the_nested_thread_before_the_one_that_started_it() {
    let mut scenario = setup();
    start_both(&mut scenario);

    assert_eq!(
        scenario.resume(SECOND),
        "suspended",
        "a thread whose built-in is still not ready suspends again"
    );
    scenario.make_ready(2);
    assert_eq!(
        scenario.resume(SECOND),
        "finished with [I32(21)]",
        "the second thread's shim found the built-in ready and returned \
         what its finish computed, through the middle instance"
    );
    scenario.make_ready(1);
    assert_eq!(scenario.resume(FIRST), "finished with [I32(11)]");
}

#[wcmp_macros::test]
fn it_resumes_the_thread_that_started_another_before_the_nested_one() {
    let mut scenario = setup();
    start_both(&mut scenario);

    scenario.make_ready(1);
    scenario.make_ready(2);
    assert_eq!(scenario.resume(FIRST), "finished with [I32(11)]");
    assert_eq!(
        scenario.resume(SECOND),
        "finished with [I32(21)]",
        "the nested thread outlived the thread and the host frame that \
         started it"
    );
}

#[wcmp_macros::test]
fn it_refuses_to_resume_a_thread_that_is_not_suspended() {
    let mut scenario = setup();
    scenario.make_ready(1);
    scenario.make_ready(2);
    assert_eq!(scenario.start_first(1), "finished with [I32(11)]");
    assert_eq!(
        scenario.spawned().as_deref(),
        Some("finished with [I32(21)]"),
        "a thread whose built-in is ready at once never suspends"
    );

    let mut context = scenario.store.internal().context();
    assert!(
        scenario.provider.resume(&mut context, FIRST).is_err(),
        "the first thread finished, so its slot holds no continuation"
    );
}

#[wcmp_macros::test]
fn it_drops_a_store_with_suspended_threads() {
    let mut scenario = setup();
    start_both(&mut scenario);

    // Nothing resumes either thread: the store drops with both in
    // the switch module's table, and the continuations go with it.
    drop(scenario);

    let mut again = setup();
    again.make_ready(1);
    again.make_ready(2);
    assert_eq!(again.start_first(1), "finished with [I32(11)]");
}

/// Thread entries of three types: one with no parameters and no
/// results, one with several of each, which calls the blocking
/// built-in, and one over floats. The second shim, over an `i64` and
/// an `f32`, answers two results.
const ENTRY_TYPES: &[u8] = wasm!(
    r#"
    (module
      (import "switch" "block" (func $block (param i32) (result i32)))
      (import "switch" "pair" (func $pair (param i64 f32) (result i64 f64)))
      (func (export "none"))
      (func (export "several") (param i32 i64) (result i32 i64 f64)
        (i32.add (call $block (local.get 0)) (i32.const 1))
        (call $pair (local.get 1) (f32.const 0.5)))
      (func (export "floats") (param f32 f64) (result f32)
        (f32.add (local.get 0) (f32.demote_f64 (local.get 1)))))
    "#
);

#[wcmp_macros::test]
fn it_encodes_an_extension_with_several_shims_and_entry_types() {
    // One extension module with two shims and two entry types, one
    // of which has no results and one several, compiles against the
    // engine that runs the provider.
    let engine = Engine::new().expect("engine");
    let mut module = SwitchModule::new(SwitchForm::StackSwitching);
    module.shim(i32_to_i32());
    module.shim(FuncType::new(
        [RuntimeValType::I64, RuntimeValType::F32],
        [RuntimeValType::I64, RuntimeValType::F64],
    ));
    module.entry(FuncType::new([], []));
    module.entry(FuncType::new(
        [RuntimeValType::I32, RuntimeValType::I64],
        [
            RuntimeValType::I32,
            RuntimeValType::I64,
            RuntimeValType::F64,
        ],
    ));
    RuntimeModule::new(engine.inner(), &module.encode()).expect("the extension compiles");
    RuntimeModule::new(engine.inner(), &SwitchModule::base()).expect("the base compiles");
}

#[wcmp_macros::test]
fn it_runs_entries_with_no_results_and_with_several() {
    // Each entry type gets an extension of its own the first time a
    // thread of that type starts, and every one of them runs on the
    // base module's workers: a finished entry hands its results over
    // whatever their number and types, and one that suspended hands
    // them over when it finishes after a resume.
    let engine = Engine::new().expect("engine");
    let mut store = Store::new(&engine, ()).expect("store");
    let mut context = store.internal().context();
    let ready: Ready = Arc::default();

    let tried = ready.clone();
    let try_block = RuntimeFunc::new(
        context.internal().runtime_mut(),
        i32_to_i32(),
        move |_store, args, results| {
            let [RuntimeVal::I32(key)] = args else {
                anyhow::bail!("the try part takes one key");
            };
            let here = tried.lock().expect("ready keys").contains(key);
            results[0] = RuntimeVal::I32(i32::from(here));
            Ok(())
        },
    );
    let finish_block = RuntimeFunc::new(
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
    let pair_type = FuncType::new(
        [RuntimeValType::I64, RuntimeValType::F32],
        [RuntimeValType::I64, RuntimeValType::F64],
    );
    let try_pair = RuntimeFunc::new(
        context.internal().runtime_mut(),
        FuncType::new(
            [RuntimeValType::I64, RuntimeValType::F32],
            [RuntimeValType::I32],
        ),
        |_store, _args, results| {
            results[0] = RuntimeVal::I32(1);
            Ok(())
        },
    );
    let finish_pair = RuntimeFunc::new(
        context.internal().runtime_mut(),
        pair_type.clone(),
        |_store, args, results| {
            let [RuntimeVal::I64(whole), RuntimeVal::F32(part)] = args else {
                anyhow::bail!("the pair takes an i64 and an f32");
            };
            results[0] = RuntimeVal::I64(whole + 1);
            results[1] = RuntimeVal::F64(f64::from(*part) * 2.0);
            Ok(())
        },
    );

    let provider =
        StackSwitchingProvider::instantiate(&mut context, engine.inner(), engine.switch_modules())
            .expect("the engine instantiates the base switch module");
    let shims = provider
        .shims(
            &mut context,
            &[
                (i32_to_i32(), try_block, finish_block),
                (pair_type, try_pair, finish_pair),
            ],
        )
        .expect("the engine instantiates the extension with both shims");
    let mut imports = Imports::default();
    imports.define("switch", "block", RuntimeExtern::Func(shims[0].clone()));
    imports.define("switch", "pair", RuntimeExtern::Func(shims[1].clone()));
    let entries = RuntimeInstance::new(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), ENTRY_TYPES).expect("the module compiles"),
        &imports,
    )
    .expect("the entries instance");
    let mut entry = |name: &str| {
        entries
            .get_export(context.internal().runtime(), name)
            .and_then(RuntimeExtern::into_func)
            .expect("an entry export")
    };
    let (none, several, floats) = (entry("none"), entry("several"), entry("floats"));

    let status = provider
        .start(&mut context, FIRST, &none, &[])
        .expect("the entry with nothing starts");
    assert_eq!(describe(&status), "finished with []");

    let status = provider
        .start(
            &mut context,
            SECOND,
            &several,
            &[RuntimeVal::I32(4), RuntimeVal::I64(7)],
        )
        .expect("the entry with several starts");
    assert_eq!(describe(&status), "suspended");
    ready.lock().expect("ready keys").insert(4);
    let status = provider
        .resume(&mut context, SECOND)
        .expect("the entry with several resumes");
    assert_eq!(
        describe(&status),
        "finished with [I32(41), I64(8), F64(1.0)]",
        "the three results crossed, after the thread suspended once"
    );

    let status = provider
        .start(
            &mut context,
            FIRST,
            &floats,
            &[RuntimeVal::F32(1.5), RuntimeVal::F64(2.0)],
        )
        .expect("the entry over floats starts");
    assert_eq!(describe(&status), "finished with [F32(3.5)]");
    assert_eq!(
        provider.workers(&mut context).expect("the worker count"),
        1,
        "the three threads ran one after another on one worker"
    );
}
