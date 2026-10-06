// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the switch module in its host-suspension form,
//! next to real core instances, on a backend that declares host
//! suspension: the browser's, and Wasmi natively.
//!
//! The host-suspension provider starts each thread entry as a
//! resumable call of the switch module's start, and a thread suspends
//! in a shim of the switch module, through a suspending host function,
//! when the blocking built-in it stands for is not ready. The tests run
//! the provider with two guest core instances around it: an entry of
//! the first calls a function of the second, which calls the shim, so
//! a frame of another instance lies between the thread's entry and its
//! suspension. A plain host import that runs inside the first thread
//! starts a second thread through the provider, as a trampoline starts
//! a nested start: the resumable call begins a second stack, the
//! second thread suspends, the start returns to the host function, and
//! the host function returns to the first thread, which then suspends
//! too. The backend keeps both calls, and the tests resume them in
//! either order.
//!
//! A start or a resume made where the store runs no guest code is the
//! store's flight, which a driver awaits: the tests await it as a
//! driver does.

use std::collections::HashSet;
use std::future::poll_fn;
use std::sync::{Arc, Mutex};

use wcmp_macros::wasm;

use crate::concurrency::{EntryStatus, HostSuspensionProvider, SuspendProvider, ThreadId};
use crate::internal::EngineInternal;
use crate::runtime_layer::{
    Extern as RuntimeExtern, Func as RuntimeFunc, FuncType, Imports, Module as RuntimeModule,
    Val as RuntimeVal, ValType as RuntimeValType, host_func, instantiate, test_suspending_backend,
};
use crate::store::{StoreContext, StoreContextInternalExt, StoreInternalExt};
use crate::{Engine, Error, Store, ThreadCause};

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
    provider: HostSuspensionProvider,
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

async fn setup() -> Scenario {
    let engine = Engine::with_backend(test_suspending_backend()).expect("engine");
    let mut store = Store::new(&engine, ()).expect("store");
    let mut context = store.internal().context();
    let ready: Ready = Arc::default();
    let tries: Arc<Mutex<u32>> = Arc::default();
    let spawned: Spawned = Arc::default();

    // The try and the finish of the blocking built-in the shim
    // stands for.
    let tried = ready.clone();
    let counted = tries.clone();
    let try_part = host_func(
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
    )
    .expect("the try part");
    let finish_part = host_func(
        context.internal().runtime_mut(),
        i32_to_i32(),
        move |_store, args, results| {
            let [RuntimeVal::I32(key)] = args else {
                anyhow::bail!("the finish part takes one key");
            };
            results[0] = RuntimeVal::I32(key * 10);
            Ok(())
        },
    )
    .expect("the finish part");

    let provider = HostSuspensionProvider::instantiate(&mut context, engine.switch_modules())
        .expect("the backend declares host suspension");
    let block = provider
        .shims(&mut context, &[(i32_to_i32(), try_part, finish_part)])
        .await
        .expect("the backend instantiates the switch module")
        .remove(0);
    provider
        .prepare_entries(&mut context, &[i32_to_i32()])
        .await
        .expect("the backend instantiates the starts");

    let mut imports = Imports::default();
    imports.define("switch", "block", block);
    let middle = instantiate(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), MIDDLE).expect("the module compiles"),
        &imports,
    )
    .await
    .expect("the middle instance");

    // `spawn` is a plain import, not a suspending one. It starts the
    // second thread as a nested start, from a host frame that runs
    // inside the first thread, and returns once the start returns.
    let second: Arc<Mutex<Option<RuntimeFunc>>> = Arc::default();
    let spawn = {
        let provider = provider.clone();
        let second = second.clone();
        let spawned = spawned.clone();
        host_func(
            context.internal().runtime_mut(),
            FuncType::new([RuntimeValType::I32], []),
            move |runtime, args, _results| {
                let entry = second
                    .lock()
                    .expect("the second entry")
                    .expect("the second entry is set before any thread runs");
                let mut context = StoreContext::new(runtime);
                let status = provider.start(&mut context, SECOND, &entry, &i32_to_i32(), args)?;
                *spawned.lock().expect("record") = Some(describe(&status));
                Ok(())
            },
        )
        .expect("spawn")
    };

    let mut imports = Imports::default();
    imports.define(
        "other",
        "middle",
        middle
            .get_export(context.internal().runtime_mut(), "middle")
            .expect("the export")
            .expect("middle"),
    );
    imports.define("host", "spawn", spawn);
    let entries = instantiate(
        context.internal().runtime_mut(),
        &RuntimeModule::new(engine.inner(), ENTRIES).expect("the module compiles"),
        &imports,
    )
    .await
    .expect("the entries instance");
    let mut entry = |name: &str| {
        entries
            .get_export(context.internal().runtime_mut(), name)
            .expect("the export")
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

/// Await the flight a start or a resume of `thread` left, as a driver
/// does, and answer where the thread stopped.
async fn stop_of(
    provider: &HostSuspensionProvider,
    context: &mut StoreContext<'_, ()>,
    thread: ThreadId,
    status: EntryStatus,
) -> crate::error::Result<EntryStatus> {
    if !matches!(status, EntryStatus::Running) {
        return Ok(status);
    }
    provider.fly(context).await;
    poll_fn(|poll| SuspendProvider::poll_stop(provider, context, thread, poll.waker())).await
}

impl Scenario {
    async fn start_first(&mut self, key: i32) -> String {
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
        let status = stop_of(&self.provider, &mut context, FIRST, status)
            .await
            .expect("the first thread stops");
        describe(&status)
    }

    async fn resume(&mut self, thread: ThreadId) -> crate::error::Result<String> {
        let mut context = self.store.internal().context();
        let status = SuspendProvider::resume(&self.provider, &mut context, thread)?;
        let status = stop_of(&self.provider, &mut context, thread, status).await?;
        Ok(describe(&status))
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
async fn start_both(scenario: &mut Scenario) {
    assert_eq!(
        scenario.start_first(1).await,
        "suspended",
        "the first thread suspended in the shim, with a frame of the \
         middle instance between its entry and the shim"
    );
    assert_eq!(
        scenario.spawned().as_deref(),
        Some("suspended"),
        "the second thread, started from a plain import inside the first, \
         suspended on a stack of its own, and its start returned to the \
         import, which returned to the first thread before that thread \
         suspended"
    );
}

#[wcmp_macros::test]
async fn it_resumes_the_nested_thread_before_the_one_that_started_it() {
    let mut scenario = setup().await;
    start_both(&mut scenario).await;

    assert_eq!(
        scenario.resume(SECOND).await.expect("the second resumes"),
        "suspended",
        "a thread whose built-in is still not ready suspends again"
    );
    scenario.make_ready(2);
    assert_eq!(
        scenario.resume(SECOND).await.expect("the second resumes"),
        "finished with [I32(21)]",
        "the second thread's shim found the built-in ready and returned \
         what its finish computed, through the middle instance"
    );
    scenario.make_ready(1);
    assert_eq!(
        scenario.resume(FIRST).await.expect("the first resumes"),
        "finished with [I32(11)]"
    );
}

#[wcmp_macros::test]
async fn it_resumes_the_thread_that_started_another_before_the_nested_one() {
    let mut scenario = setup().await;
    start_both(&mut scenario).await;

    scenario.make_ready(1);
    scenario.make_ready(2);
    assert_eq!(
        scenario.resume(FIRST).await.expect("the first resumes"),
        "finished with [I32(11)]"
    );
    assert_eq!(
        scenario.resume(SECOND).await.expect("the second resumes"),
        "finished with [I32(21)]",
        "the nested thread outlived the thread and the host frame that \
         started it"
    );
}

#[wcmp_macros::test]
async fn it_returns_from_a_shim_whose_built_in_is_ready_without_a_suspension() {
    // A call of a suspending import can always suspend: in the
    // browser, Chromium suspends a stack at every call of one. So a
    // shim that called it here would leave the thread suspended, and
    // the start would answer so. The shim calls it only when the
    // built-in is not ready.
    let mut scenario = setup().await;
    scenario.make_ready(1);
    scenario.make_ready(2);

    assert_eq!(
        scenario.start_first(1).await,
        "finished with [I32(11)]",
        "the resumable call ran the thread to its end with no suspension"
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
    assert!(
        matches!(
            scenario.resume(FIRST).await,
            Err(Error::Thread(ThreadCause::NotSuspended))
        ),
        "the first thread finished, so no call of it waits, and the resume \
         is refused with the not-suspended cause"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_resume_while_another_resume_is_under_way() {
    // A resume leaves the store a flight that a driver awaits, and the
    // store runs guest code until the flight stops. A second resume
    // made before then is refused, and leaves its thread suspended for
    // a later resume.
    let mut scenario = setup().await;
    start_both(&mut scenario).await;
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
    let first = stop_of(&scenario.provider, &mut context, FIRST, resumed)
        .await
        .expect("the first thread stops");
    assert_eq!(describe(&first), "finished with [I32(11)]");
    assert_eq!(
        scenario.resume(SECOND).await.expect("the second resumes"),
        "finished with [I32(21)]",
        "the refused resume left the second thread suspended"
    );
}

#[wcmp_macros::test]
async fn it_starts_an_entry_of_a_type_no_instantiation_prepared_only_where_the_backend_instantiates_at_once()
 {
    // The provider keeps a start for each entry type an instantiation
    // prepared. A start of any other type makes the start module then,
    // which only a backend that instantiates at once can do inside the
    // call. Wasmi can, and runs the entry. The browser instantiates on a
    // promise, so the start is refused with a structured error rather
    // than run.
    let mut scenario = setup().await;
    let mut context = scenario.store.internal().context();
    let entry = host_func(
        context.internal().runtime_mut(),
        FuncType::new([], []),
        |_store, _args, _results| Ok(()),
    )
    .expect("the entry");
    let started =
        scenario
            .provider
            .start(&mut context, SECOND, &entry, &FuncType::new([], []), &[]);
    #[cfg(target_arch = "wasm32")]
    {
        let err = started.expect_err("the browser cannot make the start in the call");
        assert!(
            err.to_string()
                .contains("a thread entry of a type no instantiation prepared"),
            "got {err}"
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let status = started.expect("Wasmi makes the start in the call");
        let status = stop_of(&scenario.provider, &mut context, SECOND, status)
            .await
            .expect("the entry stops");
        assert_eq!(describe(&status), "finished with []");
    }
}

#[wcmp_macros::test]
async fn it_drops_a_store_with_suspended_threads() {
    let mut scenario = setup().await;
    start_both(&mut scenario).await;

    // Nothing resumes either thread: the store and the provider drop
    // with both threads suspended, and the backend's calls go with
    // them.
    drop(scenario);

    let mut again = setup().await;
    again.make_ready(1);
    again.make_ready(2);
    assert_eq!(again.start_first(1).await, "finished with [I32(11)]");
}
