//! Baseline tests for the store's task, subtask, and thread records.
//!
//! Every crossing pushes a scope: a host call into a synchronous
//! export pushes the export's task, a guest call into a host function
//! pushes the subtask of that call, and a synchronous call between
//! two composed components pushes the callee's task. The tests read
//! the store's records from inside a host function, which runs while
//! the crossings they describe are on the stack, and again after the
//! call, when every scope must be gone.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    Component, Engine, FunctionParameter, FunctionType, HostCall, InterfaceIdentifier, Linker,
    PrimitiveType, ResourceType, Result, Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// What a host function saw of the store's records while it ran.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Seen {
    /// How deep the stack of current scopes was.
    scopes: usize,
    /// How many task records the store held.
    tasks: usize,
    /// How many subtask records the store held.
    subtasks: usize,
    /// How many thread records the store held.
    threads: usize,
    /// Whether the current scope was a subtask.
    current_is_subtask: bool,
    /// The component instances marked as ones that may not suspend,
    /// by their index in the store's list of instance records.
    may_not_suspend: Vec<usize>,
    /// The instance of the innermost task on the stack, by the same
    /// index. While a composed call runs that is the callee, so the
    /// two fields together say which side of the call the enter
    /// intrinsic flagged.
    current_task_instance: Option<usize>,
}

/// A component whose export calls a host function, so the host can
/// read the store's records from inside the call.
const CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core module $m
        (import "" "probe" (func $probe (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          local.get 0 call $probe i32.const 1 i32.add))
      (core instance $i (instantiate $m
        (with "" (instance (export "probe" (func $probe'))))))
      (func (export "run") (param "x" u32) (result u32)
        (canon lift (core func $i "run"))))
    "#
);

/// Two composed components: `$B`'s export calls `$A`'s export through
/// an adapter, and `$A` calls the host function, so the host reads
/// the records with the callee's task on the stack.
const COMPOSED: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (component $A
        (import "probe" (func $probe (param "x" u32) (result u32)))
        (core func $probe' (canon lower (func $probe)))
        (core module $m
          (import "" "probe" (func $probe (param i32) (result i32)))
          (func (export "double") (param i32) (result i32)
            local.get 0 call $probe i32.const 2 i32.mul))
        (core instance $i (instantiate $m
          (with "" (instance (export "probe" (func $probe'))))))
        (func (export "double") (param "x" u32) (result u32)
          (canon lift (core func $i "double"))))
      (component $B
        (import "double" (func $double (param "x" u32) (result u32)))
        (core func $double' (canon lower (func $double)))
        (core module $m
          (import "" "double" (func $double (param i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            local.get 0 call $double i32.const 1 i32.add))
        (core instance $i (instantiate $m
          (with "" (instance (export "double" (func $double'))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $A (with "probe" (func $probe))))
      (instance $b (instantiate $B (with "double" (func $a "double"))))
      (export "run" (func $b "run")))
    "#
);

/// Instantiate `bytes` with a host `probe` function that records what
/// the store's scopes looked like while it ran, and call the `run`
/// export with `argument`. Returns what the host saw during the call
/// and what the store holds after it.
async fn run_with_probe(bytes: &[u8], argument: u32) -> (Val, Seen, Seen) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let tables = store.tables.clone();
    let during: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Seen::default()));
    let recorded = during.clone();

    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().func_wrap(
        "probe",
        move |_: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
            let guard = tables.lock().expect("handle tables");
            *recorded.lock().expect("record") = Seen {
                scopes: guard.tasks.scopes().len(),
                tasks: guard.tasks.task_count(),
                subtasks: guard.tasks.subtask_count(),
                threads: guard.tasks.thread_count(),
                current_is_subtask: guard.tasks.current_subtask().is_some(),
                may_not_suspend: guard
                    .tasks
                    .instances()
                    .iter()
                    .enumerate()
                    .filter(|(_, record)| record.may_not_suspend)
                    .map(|(index, _)| index)
                    .collect(),
                current_task_instance: guard
                    .tasks
                    .current_task()
                    .and_then(|task| guard.tasks.task(task))
                    .map(|record| record.instance.index() as usize),
            };
            Ok(x)
        },
    );

    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let result = run
        .call(&mut store, &[Val::U32(argument)])
        .await
        .expect("call run");

    let guard = store.tables.lock().expect("handle tables");
    let after = Seen {
        scopes: guard.tasks.scopes().len(),
        tasks: guard.tasks.task_count(),
        subtasks: guard.tasks.subtask_count(),
        threads: guard.tasks.thread_count(),
        current_is_subtask: guard.tasks.current_subtask().is_some(),
        may_not_suspend: guard
            .tasks
            .instances()
            .iter()
            .enumerate()
            .filter(|(_, record)| record.may_not_suspend)
            .map(|(index, _)| index)
            .collect(),
        current_task_instance: guard
            .tasks
            .current_task()
            .and_then(|task| guard.tasks.task(task))
            .map(|record| record.instance.index() as usize),
    };
    drop(guard);
    let during = during.lock().expect("record").clone();
    (result.first().cloned().expect("one result"), during, after)
}

#[wcmp_macros::test]
async fn it_pushes_a_task_for_the_export_and_a_subtask_for_the_host_call() {
    let (result, during, after) = run_with_probe(CALLS_THE_HOST, 20).await;
    assert_eq!(result, Val::U32(21), "the export returned");
    assert_eq!(
        during,
        Seen {
            scopes: 2,
            tasks: 1,
            subtasks: 1,
            threads: 1,
            current_is_subtask: true,
            may_not_suspend: Vec::new(),
            current_task_instance: Some(0),
        },
        "the export's task is on the stack with the host call's subtask on top of it"
    );
    assert_eq!(
        after,
        Seen::default(),
        "both scopes are popped and their records are gone"
    );
}

#[wcmp_macros::test]
async fn it_pushes_the_callees_task_for_a_call_between_two_components() {
    let (result, during, after) = run_with_probe(COMPOSED, 20).await;
    assert_eq!(result, Val::U32(41), "the composed call returned");
    assert_eq!(
        during,
        Seen {
            scopes: 3,
            tasks: 2,
            subtasks: 1,
            threads: 2,
            current_is_subtask: true,
            may_not_suspend: vec![1],
            current_task_instance: Some(1),
        },
        "the callee's task sits on the caller's, and the host call's subtask on top of both"
    );
    assert!(
        !during.may_not_suspend.contains(&0),
        "instance 0 is $b, whose export the host called and whose task is the \
         outer one; the enter intrinsic flags the callee it is passed, not the \
         caller"
    );
    assert_eq!(
        after,
        Seen::default(),
        "the exit intrinsic pops the callee's task and restores its instance's flag"
    );
}

/// A component whose export lends an owned handle to a host call and
/// passes an invalid scalar with it. The host function takes a
/// `borrow` and a `char`; `0xD800` is a surrogate, which is not a
/// scalar value, so the lift of the second parameter fails after the
/// first has already raised the lend count on the owning entry. The
/// export keeps the handle so that a second export can drop it after
/// the failed call.
const LENDS_THEN_FAILS: &[u8] = component!(
    r#"
    (component
      (import "pdd018-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))
        (export "take" (func (param "h" (borrow $thing)) (param "c" char)))))
      (alias export $i "thing" (type $thing))
      (alias export $i "take" (func $take))
      (core func $core-take (canon lower (func $take)))
      (core func $core-drop (canon resource.drop $thing))
      (core module $m
        (import "host" "take" (func $take (param i32 i32)))
        (import "host" "drop" (func $drop (param i32)))
        (global $held (mut i32) (i32.const 0))
        (func (export "lend") (param i32)
          local.get 0 global.set $held
          local.get 0 i32.const 0xD800 call $take)
        (func (export "release")
          global.get $held call $drop))
      (core instance $c (instantiate $m
        (with "host" (instance
          (export "take" (func $core-take))
          (export "drop" (func $core-drop))))))
      (func (export "lend") (param "h" (own $thing))
        (canon lift (core func $c "lend")))
      (func (export "release")
        (canon lift (core func $c "release"))))
    "#
);

#[wcmp_macros::test]
async fn it_gives_back_a_borrow_lent_to_a_host_call_whose_parameter_lift_failed() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LENDS_THEN_FAILS)
        .await
        .expect("component parses");
    let dropped: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = dropped.clone();

    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "pdd018-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&iface)
        .resource("thing", move |_: &mut (), rep: u32| {
            recorded.lock().expect("record").push(rep);
            Ok(())
        });
    linker.instance(&iface).func_new(
        "take",
        FunctionType {
            parameters: vec![
                FunctionParameter {
                    name: "h".to_owned(),
                    ty: ValueType::Borrow(ResourceType::new("thing")),
                },
                FunctionParameter {
                    name: "c".to_owned(),
                    ty: ValueType::Primitive(PrimitiveType::Char),
                },
            ],
            result: None,
        },
        |_: HostCall<'_, ()>, _args, _results| {
            panic!("the second parameter never lifts, so the host body never runs")
        },
    );

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let handle = store.resource_new(type_id, 13).expect("mint");
    let lend = instance.get_func("lend").expect("lend export");
    lend.call(&mut store, &[Val::Own(handle)])
        .await
        .expect_err("the surrogate is not a scalar value");

    {
        let guard = store.tables.lock().expect("handle tables");
        assert!(
            guard.tasks.scopes().is_empty(),
            "the failed host call leaves no scope on the stack"
        );
        assert_eq!(guard.tasks.task_count(), 0, "no task record is left");
        assert_eq!(guard.tasks.subtask_count(), 0, "no subtask record either");
        assert_eq!(guard.tasks.thread_count(), 0, "nor any thread record");
    }

    let release = instance.get_func("release").expect("release export");
    release
        .call(&mut store, &[])
        .await
        .expect("the owning handle is no longer lent, so the guest can drop it");
    assert_eq!(
        dropped.lock().expect("record").as_slice(),
        &[13],
        "the destructor ran for the handle the failed call had lent"
    );
}
