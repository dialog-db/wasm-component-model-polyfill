//! The smoke test: one host program that tells, chapter by chapter,
//! what a developer does with the polyfill in a project of their own,
//! and reports how each story went. It sets up an engine and a store,
//! loads a component a real toolchain built and calls it, runs a
//! `wac` composition, lends the guest host functions, host resources,
//! and a core module, moves maps, fixed-length lists, and a
//! wit-bindgen world of records, variants, and resources across the
//! boundary, crosses into a 64-bit memory, walks exports by name,
//! opts into a gated feature, awaits outside the store, and learns
//! which host a WASI 0.3 HTTP handler needs. It runs as a native
//! binary (`tests smoke native`) and as a page in the browser
//! (`tests smoke web`) from the same source, so a reader can check the
//! polyfill by reading this file and by running it on both targets.
//!
//! Each story is self-contained: it builds its own store, runs a
//! component, and returns the evidence it observed. A failure in one
//! story does not stop the others, and the report reaches a
//! [`Reporter`] a story at a time, so the page shows each as it
//! completes.

mod clock;
mod host_state;
mod outcome;
mod outside;
mod reporter;
mod step;
mod story;

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_component_model_polyfill::{
    Component, CoreExternType, Engine, EngineConfig, Error, HostCall, InterfaceIdentifier,
    LinkError, Linker, Store, Val, ValField, ValueType,
};
use wcmp_macros::component;

pub use crate::host_state::HostState;
pub use crate::outcome::Outcome;
pub use crate::outside::Outside;
pub use crate::reporter::Reporter;
pub use crate::step::Step;
pub use crate::story::Story;

/// The `guest` fixture: a component `wasm-tools component new` built
/// from a core module and its WIT. It exports `double`.
const GUEST: &[u8] =
    include_bytes!("../../wasm-component-model-polyfill/tests/corpus/fixtures/guest/guest.wasm");

/// The `composition` fixture: two such components joined by `wac plug`.
/// The socket's `run` calls the plug's `double` through an adapter.
const COMPOSITION: &[u8] = include_bytes!(
    "../../wasm-component-model-polyfill/tests/corpus/fixtures/composition/composed.wasm"
);

/// The `maps` fixture: a `wasm-tools` build whose exports take and
/// return a `map<string, u32>`.
const MAPS: &[u8] =
    include_bytes!("../../wasm-component-model-polyfill/tests/corpus/fixtures/maps/maps.wasm");

/// The `fixed-lists` fixture: a `wasm-tools` build whose exports take
/// and return a `list<u32, 4>` and a `list<u8, 16>`.
const FIXED_LISTS: &[u8] = include_bytes!(
    "../../wasm-component-model-polyfill/tests/corpus/fixtures/fixed-lists/fixed-lists.wasm"
);

/// The `rich` fixture: three components `cargo` and wit-bindgen
/// built against a world of records, variants, enums, flags,
/// options, results, nested lists, strings, and two resources,
/// joined by two `wac plug` steps. Every call it answers has crossed
/// three component boundaries and come back.
const RICH: &[u8] =
    include_bytes!("../../wasm-component-model-polyfill/tests/corpus/fixtures/rich/rich.wasm");

/// The `wasi-http` fixture: a WASI 0.3 HTTP handler the same
/// toolchain built. Its export is an `async func` whose request and
/// response carry a `stream<u8>` body and a `future` of trailers,
/// whose types it imports from `wasi:http/types`.
const WASI_HTTP: &[u8] = include_bytes!(
    "../../wasm-component-model-polyfill/tests/corpus/fixtures/wasi-http/handler.wasm"
);

/// A component that exports a core module for the host to take: one
/// global and one function, and nothing the component instantiates
/// itself.
const MODULE_PROVIDER: &[u8] = component!(
    r#"
    (component
      (core module $m
        (global (export "g") i32 i32.const 100)
        (func (export "f") (result i32) i32.const 101))
      (export "m" (core module $m)))
    "#
);

/// A component that imports that core module, instantiates it, and
/// exports the sum of its function's result and its global.
const MODULE_CONSUMER: &[u8] = component!(
    r#"
    (component
      (import "m" (core module $m
        (export "f" (func (result i32)))
        (export "g" (global i32))))
      (core instance $provided (instantiate $m))
      (core module $sum
        (import "m" "f" (func $f (result i32)))
        (import "m" "g" (global $g i32))
        (func (export "sum") (result i32)
          call $f global.get $g i32.add))
      (core instance $i (instantiate $sum (with "m" (instance $provided))))
      (func (export "sum") (result u32) (canon lift (core func $i "sum"))))
    "#
);

/// A component whose functions live under a plain-named instance
/// export, `a`, and under an instance nested inside it, `a.b`.
const NESTED_EXPORTS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "f") (result i32) i32.const 42)
        (func (export "g") (param i32) (result i32) local.get 0 i32.const 1 i32.add))
      (core instance $i (instantiate $m))
      (func $f (result u32) (canon lift (core func $i "f")))
      (func $g (param "x" u32) (result u32) (canon lift (core func $i "g")))
      (instance $b (export "g" (func $g)))
      (instance $a (export "f" (func $f)) (export "b" (instance $b)))
      (export "a" (instance $a)))
    "#
);

/// A 32-bit component that forwards a string to a 64-bit component,
/// which copies it inside its `i64`-addressed memory and hands it
/// back, so the string crosses an adapter in each direction.
const MEMORY64_COMPOSITION: &[u8] = component!(
    r#"
    (component
      (component $c64
        (core module $m
          (memory (export "memory") i64 1)
          (global $next (mut i64) (i64.const 8))
          (func $realloc (export "realloc")
            (param $old i64) (param $old-size i64) (param $align i64) (param $size i64)
            (result i64)
            (local $ret i64)
            (local.set $ret
              (i64.and (i64.add (global.get $next) (i64.const 7)) (i64.const -8)))
            (global.set $next (i64.add (local.get $ret) (local.get $size)))
            (local.get $ret))
          (func (export "roundtrip") (param $ptr i64) (param $len i64) (result i64)
            (local $dst i64)
            (local $ret i64)
            (local.set $dst
              (call $realloc (i64.const 0) (i64.const 0) (i64.const 1) (local.get $len)))
            (memory.copy (local.get $dst) (local.get $ptr) (local.get $len))
            (local.set $ret
              (call $realloc (i64.const 0) (i64.const 0) (i64.const 8) (i64.const 16)))
            (i64.store (local.get $ret) (local.get $dst))
            (i64.store offset=8 (local.get $ret) (local.get $len))
            (local.get $ret)))
        (core instance $m (instantiate $m))
        (func (export "roundtrip") (param "a" string) (result string)
          (canon lift (core func $m "roundtrip")
            (memory (core memory $m "memory"))
            (realloc (core func $m "realloc")))))
      (instance $c64 (instantiate $c64))
      (component $c32
        (import "backend" (instance $i
          (export "roundtrip" (func (param "a" string) (result string)))))
        (core module $libc
          (memory (export "memory") 1)
          (global $next (mut i32) (i32.const 8))
          (func (export "realloc")
            (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
            (result i32)
            (local $ret i32)
            (local.set $ret
              (i32.and (i32.add (global.get $next) (i32.const 7)) (i32.const -8)))
            (global.set $next (i32.add (local.get $ret) (local.get $size)))
            (local.get $ret)))
        (core instance $libc (instantiate $libc))
        (core func $roundtrip
          (canon lower (func $i "roundtrip")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc"))))
        (core module $m
          (import "" "memory" (memory 1))
          (import "" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
          (import "" "roundtrip" (func $roundtrip (param i32 i32 i32)))
          (func (export "roundtrip") (param $ptr i32) (param $len i32) (result i32)
            (local $ret i32)
            (local.set $ret
              (call $realloc (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
            (call $roundtrip (local.get $ptr) (local.get $len) (local.get $ret))
            (local.get $ret)))
        (core instance $m (instantiate $m
          (with "" (instance
            (export "memory" (memory $libc "memory"))
            (export "realloc" (func $libc "realloc"))
            (export "roundtrip" (func $roundtrip))))))
        (func (export "roundtrip") (param "a" string) (result string)
          (canon lift (core func $m "roundtrip")
            (memory (core memory $libc "memory"))
            (realloc (core func $libc "realloc")))))
      (instance $c32 (instantiate $c32 (with "backend" (instance $c64))))
      (export "roundtrip" (func $c32 "roundtrip")))
    "#
);

/// A component whose import carries the gated `implements`
/// annotation, which an engine accepts only when configured to.
const IMPLEMENTS: &[u8] = component!(
    r#"
    (component (import "a" (implements "a:b/c") (instance)))
    "#
);

/// A component that imports a host function and exports functions
/// whose signatures cross the Canonical ABI in both directions:
/// strings and a list lowered into guest memory through
/// `cabi_realloc`, a string lifted back out, and a scalar handed to
/// the host.
const GREETER: &[u8] = component!(
    r#"
    (component
      (type $host (instance
        (export "tally" (func (param "n" u32)))))
      (import "wcmp:smoke/host@0.1.0" (instance $host (type $host)))
      (alias export $host "tally" (func $tally))
      (core func $core-tally (canon lower (func $tally)))
      (core module $m
        (import "host" "tally" (func $tally (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 1024))
        ;; A bump allocator that honors alignment and moves a block on
        ;; reallocation, which is what the string lowering needs.
        (func $realloc (export "cabi_realloc")
              (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
              (result i32)
          (local $ptr i32)
          global.get $bump local.get $align i32.add i32.const 1 i32.sub
          local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
          local.set $ptr
          local.get $ptr local.get $size i32.add global.set $bump
          local.get $old
          i32.eqz
          if
            local.get $ptr
            return
          end
          local.get $ptr
          local.get $old
          local.get $old-size local.get $size
          local.get $old-size local.get $size i32.lt_u
          select
          memory.copy
          local.get $ptr)
        ;; `len(s: string) -> s32`: the byte length of a lowered string.
        (func (export "len") (param i32 i32) (result i32)
          local.get 1)
        ;; `echo(s: string) -> string`: return the lowered string as the
        ;; (pointer, length) pair the lift reads.
        (func (export "echo") (param i32 i32) (result i32)
          (local $ret i32)
          i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
          local.get $ret local.get 0 i32.store
          local.get $ret local.get 1 i32.store offset=4
          local.get $ret)
        ;; `sum(xs: list<u32>) -> u32`: add up a lowered list.
        (func (export "sum") (param $ptr i32) (param $len i32) (result i32)
          (local $i i32) (local $acc i32)
          block $done
            loop $more
              local.get $i local.get $len i32.ge_u br_if $done
              local.get $acc
              local.get $ptr local.get $i i32.const 4 i32.mul i32.add i32.load
              i32.add local.set $acc
              local.get $i i32.const 1 i32.add local.set $i
              br $more
            end
          end
          local.get $acc)
        ;; `notify(n: u32)`: call the host with twice the argument.
        (func (export "notify") (param i32)
          local.get 0 i32.const 2 i32.mul call $tally))
      (core instance $i (instantiate $m
        (with "host" (instance (export "tally" (func $core-tally))))))
      (func (export "len") (param "s" string) (result s32)
        (canon lift (core func $i "len")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "echo") (param "s" string) (result string)
        (canon lift (core func $i "echo")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "sum") (param "xs" (list u32)) (result u32)
        (canon lift (core func $i "sum")
          (memory (core memory $i "memory"))
          (realloc (core func $i "cabi_realloc"))))
      (func (export "notify") (param "n" u32)
        (canon lift (core func $i "notify"))))
    "#
);

/// A component that imports a host resource type and exports a
/// function that drops two owned handles, so the host can watch its
/// destructor run once per handle, in order.
const DROPPER: &[u8] = component!(
    r#"
    (component
      (import "wcmp:smoke/resources@0.1.0" (instance $i
        (export "thing" (type (sub resource)))))
      (alias export $i "thing" (type $thing))
      (core func $thing-drop (canon resource.drop $thing))
      (core module $m
        (func (import "host" "drop") (param i32))
        (func (export "drop2") (param i32 i32)
          local.get 0 call 0
          local.get 1 call 0))
      (core instance $core (instantiate $m
        (with "host" (instance (export "drop" (func $thing-drop))))))
      (func (export "drop2") (param "a" (own $thing)) (param "b" (own $thing))
        (canon lift (core func $core "drop2"))))
    "#
);

/// A story's body: what it does against the engine, and the evidence
/// it returns or the reason it failed.
type Body<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + 'a>>;

pub static ENGINE_AND_STORE: Story = Story {
    chapter: "Getting started",
    title: "Create an engine and a store",
    goal: "You embed the polyfill in a program of your own. An engine compiles components, \
           and a store carries your host state next to every instance it holds, readable and \
           writable from your side.",
};

pub static LOAD_AND_CALL: Story = Story {
    chapter: "Getting started",
    title: "Load a component and call a typed export",
    goal: "You have a component that `wasm-tools component new` built from a core module and \
           its WIT. You load it, instantiate it, and call its `double` export with Rust types \
           on both sides.",
};

pub static COMPOSITION_STORY: Story = Story {
    chapter: "Getting started",
    title: "Run a `wac` composition",
    goal: "You joined two components with `wac plug`. The result is one component, and the \
           polyfill supplies the adapter that carries a call from the socket into the plug \
           and back.",
};

pub static HOST_FUNCTION: Story = Story {
    chapter: "Host integration",
    title: "Provide a host function and pass strings and lists",
    goal: "Your guest needs something only the host can do. You register a typed host \
           function in the linker, and strings and a list cross the canonical ABI into guest \
           memory and back out.",
};

pub static HOST_RESOURCE: Story = Story {
    chapter: "Host integration",
    title: "Hand the guest a host resource",
    goal: "You define a resource whose representation lives on your side. When the guest \
           drops a handle, your destructor runs, once per handle and in drop order.",
};

pub static DISPOSAL: Story = Story {
    chapter: "Host integration",
    title: "Release handles and tear everything down",
    goal: "You release a handle you never gave the guest, a second release is refused, and \
           the instance and the store go away without running a destructor for a handle you \
           still held.",
};

pub static CORE_MODULES: Story = Story {
    chapter: "Host integration",
    title: "Lend a core module between components",
    goal: "A component exports a plain core module. You read its shape, instantiate it \
           yourself, and register it in a linker so a second component can instantiate it \
           too.",
};

pub static MAPS_AND_FIXED_LISTS: Story = Story {
    chapter: "Real toolchains, real types",
    title: "Pass maps and fixed-length lists",
    goal: "Your WIT uses `map<string, u32>`, `list<u32, 4>`, and `list<u8, 16>`. Each \
           crosses into a component `wasm-tools` built and back, as a Rust `HashMap` or \
           array and as an untyped `Val`.",
};

pub static RICH_WORLD: Story = Story {
    chapter: "Real toolchains, real types",
    title: "Drive a wit-bindgen world with records, variants, and resources",
    goal: "Three components that `cargo` and wit-bindgen built share one world of records, \
           variants, enums, flags, options, results, nested lists, strings, and resources. \
           Every call crosses all three and comes back.",
};

pub static MEMORY64: Story = Story {
    chapter: "Real toolchains, real types",
    title: "Cross into a 64-bit memory",
    goal: "One of your components uses a 64-bit memory. A string goes from the host into a \
           32-bit component, through an adapter into an `i64`-addressed memory, and back \
           unchanged.",
};

pub static NAVIGATION: Story = Story {
    chapter: "Introspection and configuration",
    title: "Find exports by name and read their signatures",
    goal: "Your component nests functions under instance exports. You walk to `a.b.g` by \
           name and read its parameter and result types before you call it.",
};

pub static ENGINE_CONFIGURATION: Story = Story {
    chapter: "Introspection and configuration",
    title: "Opt into a gated feature",
    goal: "A component uses the `implements` annotation, which sits behind a feature gate. \
           The default engine refuses it and names the gate; an engine you configure \
           accepts it.",
};

pub static RUN_CONCURRENT: Story = Story {
    chapter: "Asynchronous hosts",
    title: "Await outside the store without blocking it",
    goal: "Inside a `run_concurrent` entry your closure reads host state, awaits a timer the \
           store knows nothing about, and writes host state when it resumes. The store sits \
           idle in between rather than reporting a deadlock.",
};

pub static WASI_HTTP_STORY: Story = Story {
    chapter: "Known limits",
    title: "Learn which host a WASI 0.3 HTTP handler needs",
    goal: "You try a `wasi:http` 0.3 handler whose request and response carry streams and \
           futures. The polyfill translates it, and without a host for `wasi:http/types` \
           it stops at link and names that import instead of failing somewhere inside.",
};

/// Every story in the order the report tells them, each with its
/// body. A body is a future and does nothing until `run` awaits it.
fn stories(engine: &Engine) -> Vec<(&'static Story, Body<'_>)> {
    vec![
        (&ENGINE_AND_STORE, Box::pin(engine_and_store(engine))),
        (&LOAD_AND_CALL, Box::pin(load_and_call(engine))),
        (&COMPOSITION_STORY, Box::pin(composition(engine))),
        (&HOST_FUNCTION, Box::pin(host_function(engine))),
        (&HOST_RESOURCE, Box::pin(host_resource(engine))),
        (&DISPOSAL, Box::pin(disposal(engine))),
        (&CORE_MODULES, Box::pin(core_modules(engine))),
        (
            &MAPS_AND_FIXED_LISTS,
            Box::pin(maps_and_fixed_lists(engine)),
        ),
        (&RICH_WORLD, Box::pin(rich_world(engine))),
        (&MEMORY64, Box::pin(memory64(engine))),
        (&NAVIGATION, Box::pin(navigation(engine))),
        (&ENGINE_CONFIGURATION, Box::pin(engine_configuration())),
        (&RUN_CONCURRENT, Box::pin(run_concurrent_outside(engine))),
        (&WASI_HTTP_STORY, Box::pin(wasi_http(engine))),
    ]
}

/// Run every story in order and return the report, telling `reporter`
/// about each chapter and story as it completes. Compiling,
/// instantiating, and calling are awaited, so the browser can compile
/// through its asynchronous API and paint between stories; natively
/// the futures complete at once.
pub async fn run(reporter: &mut impl Reporter) -> Vec<Step> {
    let engine = match Engine::new() {
        Ok(engine) => engine,
        Err(err) => {
            let step = Step {
                story: &ENGINE_AND_STORE,
                outcome: Outcome::Failed(format!("Engine::new failed: {err}")),
                millis: 0.0,
            };
            reporter.begin(1);
            reporter.chapter(step.story.chapter);
            reporter.step(&step);
            return vec![step];
        }
    };
    let stories = stories(&engine);
    reporter.begin(stories.len());
    let mut steps = Vec::with_capacity(stories.len());
    let mut chapter = "";
    for (story, body) in stories {
        if story.chapter != chapter {
            chapter = story.chapter;
            reporter.chapter(chapter);
        }
        let step = Step::run(story, body).await;
        reporter.step(&step);
        steps.push(step);
    }
    steps
}

/// A chapter's line of the transcript.
pub fn chapter_line(chapter: &str) -> String {
    format!("== {chapter}")
}

/// How many stories passed, failed, and were skipped, in that order.
pub fn counts(steps: &[Step]) -> (usize, usize, usize) {
    let count =
        |wanted: fn(&Outcome) -> bool| steps.iter().filter(|step| wanted(&step.outcome)).count();
    (
        count(|outcome| matches!(outcome, Outcome::Passed(_))),
        count(|outcome| matches!(outcome, Outcome::Failed(_))),
        count(|outcome| matches!(outcome, Outcome::Skipped(_))),
    )
}

/// The report's last line. `tests smoke check` compares it between
/// the native run and the page.
pub fn summary(steps: &[Step]) -> String {
    let (passed, failed, skipped) = counts(steps);
    format!("smoke: {passed} passed, {failed} failed, {skipped} skipped")
}

pub fn all_passed(steps: &[Step]) -> bool {
    !steps
        .iter()
        .any(|step| matches!(step.outcome, Outcome::Failed(_)))
}

fn fail(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn expect<T: PartialEq + std::fmt::Debug>(what: &str, got: T, wanted: T) -> Result<(), String> {
    if got == wanted {
        Ok(())
    } else {
        Err(format!("{what}: expected {wanted:?}, got {got:?}"))
    }
}

/// An engine and a store construct through the public API and the
/// store hands its data back.
async fn engine_and_store(engine: &Engine) -> Result<String, String> {
    let mut store: Store<HostState> = Store::new(engine, HostState::default()).map_err(fail)?;
    store.data_mut().tallies.push(7);
    expect("store data", store.data().tallies.as_slice(), &[7])?;
    Ok("engine and store constructed; store data readable and writable".to_owned())
}

/// A component built by a real toolchain loads, instantiates, and
/// answers a typed call.
async fn load_and_call(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GUEST).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let double = instance
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;
    let result = double.call(&mut store, (21,)).await.map_err(fail)?;
    expect("double(21)", result, 42)?;
    Ok(format!(
        "{} bytes of wasm-tools output; double(21) = {result}",
        GUEST.len()
    ))
}

/// Strings and a list lower into guest memory, a string lifts back
/// out, and a typed host function receives what the guest sends.
async fn host_function(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, GREETER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let host: InterfaceIdentifier = "wcmp:smoke/host@0.1.0".parse().map_err(fail)?;
    linker
        .instance(&host)
        .func_wrap(
            "tally",
            |mut state: HostCall<'_, HostState>,
             (n,): (u32,)|
             -> wasm_component_model_polyfill::Result<()> {
                state.data_mut().tallies.push(n);
                Ok(())
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;

    let len = instance
        .get_func("len")
        .ok_or("no `len` export")?
        .typed::<(String,), i32>()
        .map_err(fail)?;
    let length = len
        .call(&mut store, ("héllo".to_owned(),))
        .await
        .map_err(fail)?;
    expect("len(\"héllo\") in UTF-8 bytes", length, 6)?;

    let echo = instance
        .get_func("echo")
        .ok_or("no `echo` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let echoed = echo
        .call(&mut store, ("round trip".to_owned(),))
        .await
        .map_err(fail)?;
    expect("echo", echoed.as_str(), "round trip")?;

    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(Vec<u32>,), u32>()
        .map_err(fail)?;
    let total = sum
        .call(&mut store, (vec![1, 2, 3, 4, 5],))
        .await
        .map_err(fail)?;
    expect("sum([1..5])", total, 15)?;

    let notify = instance
        .get_func("notify")
        .ok_or("no `notify` export")?
        .typed::<(u32,), ()>()
        .map_err(fail)?;
    notify.call(&mut store, (21,)).await.map_err(fail)?;
    expect("tallies", store.data().tallies.as_slice(), &[42])?;

    Ok(format!(
        "len = {length}, echo = {echoed:?}, sum = {total}, host saw tally({})",
        store.data().tallies[0]
    ))
}

/// A host resource's destructor runs exactly once per handle the guest
/// drops, in drop order.
async fn host_resource(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let resources: InterfaceIdentifier = "wcmp:smoke/resources@0.1.0".parse().map_err(fail)?;
    let thing = linker
        .instance(&resources)
        .resource(
            "thing",
            |state: &mut HostState, rep: u32| -> wasm_component_model_polyfill::Result<()> {
                state.dropped.push(rep);
                Ok(())
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let first = store.resource_new(thing, 11).map_err(fail)?;
    let second = store.resource_new(thing, 22).map_err(fail)?;
    let drop2 = instance.get_func("drop2").ok_or("no `drop2` export")?;
    drop2
        .call(&mut store, &[Val::Own(first), Val::Own(second)])
        .await
        .map_err(fail)?;
    expect(
        "destructor order",
        store.data().dropped.as_slice(),
        &[11, 22],
    )?;
    Ok(format!(
        "destructor ran for reps {:?}, in drop order",
        store.data().dropped
    ))
}

/// The host releases a handle it never handed to the guest, watches
/// the destructor run, then drops the instance and the store.
async fn disposal(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, DROPPER).await.map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    let resources: InterfaceIdentifier = "wcmp:smoke/resources@0.1.0".parse().map_err(fail)?;
    let thing = linker
        .instance(&resources)
        .resource(
            "thing",
            |state: &mut HostState, rep: u32| -> wasm_component_model_polyfill::Result<()> {
                state.dropped.push(rep);
                Ok(())
            },
        )
        .map_err(fail)?;
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let kept = store.resource_new(thing, 33).map_err(fail)?;
    let leaked = store.resource_new(thing, 44).map_err(fail)?;
    store.resource_drop(kept).map_err(fail)?;
    expect(
        "destructor after release",
        store.data().dropped.as_slice(),
        &[33],
    )?;
    let again = store.resource_drop(kept);
    expect("a second release is refused", again.is_err(), true)?;
    // The instance can go before the store; the store keeps the
    // tables and the destructor.
    drop(instance);
    let _ = leaked;
    let dropped = store.data().dropped.clone();
    drop(store);
    expect(
        "a dropped store runs no destructor for the leaked handle",
        dropped.as_slice(),
        &[33],
    )?;
    Ok(
        "release ran the destructor once; a second release was refused; the store dropped \
        with one leaked handle and ran nothing"
            .to_owned(),
    )
}

/// A `wac` composition of two real guests runs through the adapter
/// the translator emits between them.
async fn composition(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, COMPOSITION).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let run = instance
        .get_func("run")
        .ok_or("no `run` export")?
        .typed::<(u32,), u32>()
        .map_err(fail)?;
    let result = run.call(&mut store, (20,)).await.map_err(fail)?;
    expect("run(20) = double(20) + 1", result, 41)?;
    Ok(format!(
        "{} bytes of wac output; socket.run(20) -> plug.double -> {result}",
        COMPOSITION.len()
    ))
}

/// Two components built by a real toolchain move a `map<string, u32>`
/// and fixed-length lists in both directions, typed and untyped. The
/// map's keys are sent as `Val::Map` where their order matters, so
/// the evidence is the same on every target.
async fn maps_and_fixed_lists(engine: &Engine) -> Result<String, String> {
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;

    let maps = Component::new(engine, MAPS).await.map_err(fail)?;
    let maps = linker.instantiate(&mut store, &maps).await.map_err(fail)?;
    let sum = maps
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(HashMap<String, u32>,), u32>()
        .map_err(fail)?;
    let map: HashMap<String, u32> = [("a", 1), ("b", 2), ("c", 39)]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    let total = sum.call(&mut store, (map,)).await.map_err(fail)?;
    expect("sum({a: 1, b: 2, c: 39})", total, 42)?;
    let entries = Val::Map(Box::new([
        (Val::String("x".to_owned()), Val::U32(7)),
        (Val::String("y".to_owned()), Val::U32(8)),
    ]));
    let keys = maps
        .get_func("keys")
        .ok_or("no `keys` export")?
        .call(&mut store, std::slice::from_ref(&entries))
        .await
        .map_err(fail)?;
    expect(
        "keys({x: 7, y: 8})",
        keys.as_ref(),
        &[Val::List(Box::new([
            Val::String("x".to_owned()),
            Val::String("y".to_owned()),
        ]))],
    )?;
    let back = maps
        .get_func("identity")
        .ok_or("no `identity` export")?
        .call(&mut store, std::slice::from_ref(&entries))
        .await
        .map_err(fail)?;
    expect("identity({x: 7, y: 8})", back.as_ref(), &[entries])?;

    let lists = Component::new(engine, FIXED_LISTS).await.map_err(fail)?;
    let lists = linker.instantiate(&mut store, &lists).await.map_err(fail)?;
    let sum4 = lists
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<([u32; 4],), u32>()
        .map_err(fail)?;
    let total4 = sum4
        .call(&mut store, ([1, 2, 3, 36],))
        .await
        .map_err(fail)?;
    expect("sum([1, 2, 3, 36])", total4, 42)?;
    let double = lists
        .get_func("double")
        .ok_or("no `double` export")?
        .typed::<([u8; 16],), [u8; 16]>()
        .map_err(fail)?;
    let input: [u8; 16] = core::array::from_fn(|i| i as u8);
    let doubled = double.call(&mut store, (input,)).await.map_err(fail)?;
    expect(
        "double(0..16)",
        doubled,
        core::array::from_fn(|i| 2 * i as u8),
    )?;
    let bytes = Val::FixedLengthList(input.iter().map(|b| Val::U8(*b)).collect());
    let same = lists
        .get_func("identity")
        .ok_or("no `identity` export")?
        .call(&mut store, std::slice::from_ref(&bytes))
        .await
        .map_err(fail)?;
    expect("identity(0..16)", same.as_ref(), &[bytes])?;

    Ok(format!(
        "maps.wasm ({} bytes): sum = {total}, keys = [x, y], identity kept 2 entries; \
         fixed-lists.wasm ({} bytes): sum = {total4}, double(0..16) ends in {}, identity kept 16 bytes",
        MAPS.len(),
        FIXED_LISTS.len(),
        doubled[15]
    ))
}

/// A component exports a core module; the host reads its shape,
/// instantiates it itself, and registers it for a second component
/// that instantiates it in turn.
async fn core_modules(engine: &Engine) -> Result<String, String> {
    let provider = Component::new(engine, MODULE_PROVIDER)
        .await
        .map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &provider)
        .await
        .map_err(fail)?;
    let module = instance.get_module("m").ok_or("no `m` module export")?;
    let shape: Vec<String> = module
        .exports()
        .iter()
        .map(|export| {
            let kind = match export.ty {
                CoreExternType::Func { .. } => "func",
                CoreExternType::Global { .. } => "global",
                CoreExternType::Memory { .. } => "memory",
                CoreExternType::Table { .. } => "table",
                CoreExternType::Tag { .. } => "tag",
                _ => "other",
            };
            format!("{} ({kind})", export.name)
        })
        .collect();
    expect("module imports", module.imports().len(), 0)?;
    expect(
        "module exports",
        shape.as_slice(),
        &["g (global)".to_owned(), "f (func)".to_owned()],
    )?;
    let core = module.instantiate(&mut store, &[]).await.map_err(fail)?;
    let g = core.get_export(&store, "g").ok_or("no `g` core export")?;
    expect(
        "the host reads the global's type",
        matches!(g.ty(&store), CoreExternType::Global { .. }),
        true,
    )?;

    let consumer = Component::new(engine, MODULE_CONSUMER)
        .await
        .map_err(fail)?;
    let mut linker: Linker<HostState> = Linker::new(engine);
    linker.root().module("m", &module).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &consumer)
        .await
        .map_err(fail)?;
    let sum = instance
        .get_func("sum")
        .ok_or("no `sum` export")?
        .typed::<(), u32>()
        .map_err(fail)?;
    let total = sum.call(&mut store, ()).await.map_err(fail)?;
    expect("f() + g", total, 201)?;
    Ok(format!(
        "exported module m has {} imports and exports {}; the host instantiated it, then a \
         second component instantiated it through the linker: f() + g = {total}",
        module.imports().len(),
        shape.join(", ")
    ))
}

/// Functions inside a plain-named instance export and inside a nested
/// one are reached by name, and a handle reports its signature.
async fn navigation(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, NESTED_EXPORTS).await.map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    expect("no root-level `f`", instance.get_func("f").is_none(), true)?;
    let a = instance
        .exports()
        .instance("a")
        .ok_or("no `a` instance export")?;
    let f = a.func("f").ok_or("no `a.f` export")?;
    let forty_two = f.call(&mut store, &[]).await.map_err(fail)?;
    expect("a.f()", forty_two.as_ref(), &[Val::U32(42)])?;
    let g = a
        .instance("b")
        .ok_or("no `a.b` instance export")?
        .func("g")
        .ok_or("no `a.b.g` export")?;
    let signature = g.ty().clone();
    let parameters: Vec<String> = signature
        .parameters
        .iter()
        .map(|parameter| format!("{}: {}", parameter.name, type_name(&parameter.ty)))
        .collect();
    let g = g.typed::<(u32,), u32>().map_err(fail)?;
    let result = g.call(&mut store, (41,)).await.map_err(fail)?;
    expect("a.b.g(41)", result, 42)?;
    Ok(format!(
        "a.f() = 42, a.b.g(41) = {result}, a.b.g takes ({}) and returns {}",
        parameters.join(", "),
        signature
            .result
            .as_ref()
            .map_or("nothing".to_owned(), type_name)
    ))
}

/// A value type as WIT spells it, for the primitives the smoke test
/// shows; any other shape falls back to the polyfill's debug form.
fn type_name(ty: &ValueType) -> String {
    match ty {
        ValueType::Primitive(primitive) => format!("{primitive:?}").to_lowercase(),
        other => format!("{other:?}"),
    }
}

/// A string goes from the host into a 32-bit component, through an
/// adapter into a 64-bit component that copies it in an `i64`
/// memory, and back.
async fn memory64(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, MEMORY64_COMPOSITION)
        .await
        .map_err(fail)?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;
    let roundtrip = instance
        .get_func("roundtrip")
        .ok_or("no `roundtrip` export")?
        .typed::<(String,), String>()
        .map_err(fail)?;
    let text = "héllo from a 64-bit memory";
    let back = roundtrip
        .call(&mut store, (text.to_owned(),))
        .await
        .map_err(fail)?;
    expect("roundtrip", back.as_str(), text)?;
    Ok(format!(
        "{:?} crossed into an i64 memory and back, {} bytes each way",
        text,
        text.len()
    ))
}

/// The default engine validates with Wasmtime's feature gates, and a
/// host opts into a gated feature through the engine configuration.
async fn engine_configuration() -> Result<String, String> {
    let strict = Engine::new().map_err(fail)?;
    let rejection = match Component::new(&strict, IMPLEMENTS).await {
        Ok(_) => return Err("the default engine accepted `implements`".to_owned()),
        Err(Error::InvalidComponentBinary { message, .. }) if message.contains("cm-implements") => {
            "the `cm-implements` feature is not active"
        }
        Err(other) => return Err(format!("unexpected rejection: {other}")),
    };
    let mut config = EngineConfig::new();
    config.wasm_component_model_implements(true);
    let permissive = Engine::with_config(&config).map_err(fail)?;
    let component = Component::new(&permissive, IMPLEMENTS)
        .await
        .map_err(fail)?;
    expect(
        "the annotated import is described",
        component.imports.len(),
        1,
    )?;
    Ok(format!(
        "the default engine rejected an `implements` import ({rejection}); an engine that opts \
         in accepted it"
    ))
}

/// How long the `run_concurrent` closure waits outside the store, in
/// milliseconds. The step then waits twice as long with the entry
/// unpolled, so the closure's timer has fired before the entry is
/// driven again.
const WAIT_MILLIS: u32 = 20;

/// A waker that counts the wakes it receives. A wake that lands here
/// is how a step says a wake arrived without comparing waker
/// identities, which two clones of one waker do not always agree on.
#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wakes {
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl std::task::Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Poll `future` once, as an executor would.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

/// The store's `run_concurrent` entry waits on something the store
/// cannot resolve, and comes back with the closure's value.
///
/// The closure reads the host data through the accessor, awaits a
/// timer outside the store, reads the host data again and returns.
/// While it waits the store is idle: every other driver would fail
/// with the deadlock cause there, and this one returns pending
/// instead, because the waker it was polled with is what brings it
/// back. The step polls the entry by hand once to see that, and then
/// waits twice as long with nothing polling it, so the timer's wake
/// lands on the waker the hand poll gave it.
async fn run_concurrent_outside(engine: &Engine) -> Result<String, String> {
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    store.data_mut().tallies.push(1);

    // What the host does while the wait is on: in the browser, a
    // `setTimeout` of zero asked for now, which only a page still
    // delivering callbacks will run.
    let outside = Outside::watching();

    let mut entry = Box::pin(store.run_concurrent(async |accessor| {
        let before = accessor.with(|store| store.data().tallies.len())?;
        // Nothing the store owns can resolve this.
        Outside::pause(WAIT_MILLIS).await;
        let after = accessor.with(|store| {
            store.data_mut().tallies.push(2);
            store.data().tallies.len()
        })?;
        Ok::<String, Error>(format!("{before} then {after}"))
    }));

    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    match poll_once(&mut entry, &waker) {
        Poll::Pending => (),
        Poll::Ready(Ok(_)) => return Err("the entry completed without waiting".to_owned()),
        Poll::Ready(Err(error)) => {
            return Err(format!("the entry failed instead of waiting: {error}"));
        }
    }

    Outside::pause(2 * WAIT_MILLIS).await;
    let woken = wakes.count();
    expect("the wait woke the entry's waker", woken > 0, true)?;
    let watched = outside.observed()?;

    let value = entry.await.map_err(fail)?.map_err(fail)?;
    expect("the closure's value", value.as_str(), "1 then 2")?;
    expect(
        "what the closure wrote stayed in the store",
        store.data().tallies.as_slice(),
        &[1, 2],
    )?;

    Ok(format!(
        "a hand poll found the entry pending, not deadlocked, while the closure waited \
         {WAIT_MILLIS} ms outside the store; {watched}; the timer woke the entry {woken} \
         time(s) with nothing polling it; the entry returned {value:?}, and the host data the \
         closure wrote stayed in the store as {:?}",
        store.data().tallies
    ))
}

/// The `rich` fixture: a world of records, variants, enums, flags,
/// options, results, nested lists, and strings, and a resource on
/// each side of an import. Every value crosses three component
/// boundaries — driver to guest, guest to support, and back — so one
/// call exercises wit-bindgen's lift and lower code and the
/// allocator's `cabi_realloc` six times.
async fn rich_world(engine: &Engine) -> Result<String, String> {
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    let component = Component::new(engine, RICH).await.map_err(fail)?;
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .map_err(fail)?;

    let call = |name: &'static str| {
        instance
            .get_func(name)
            .ok_or_else(|| format!("no `{name}` export"))
    };

    // An enum, a flags set, an option, and a string back.
    let described = call("describe")?
        .call(
            &mut store,
            &[
                Val::Enum("green".to_owned()),
                Val::Flags(Box::new(["bold".to_owned(), "italic".to_owned()])),
                Val::Option(Some(Box::new(Val::String("hello".to_owned())))),
            ],
        )
        .await
        .map_err(fail)?;
    expect(
        "describe(green, {bold, italic}, some(\"hello\"))",
        described.as_ref(),
        &[Val::String("green[bold,italic] hello".to_owned())],
    )?;

    // A nested list in, a flat one out.
    let folded = call("fold")?
        .call(
            &mut store,
            &[Val::List(Box::new([
                Val::List(Box::new([Val::U32(1), Val::U32(2), Val::U32(3)])),
                Val::List(Box::new([Val::U32(10)])),
            ]))],
        )
        .await
        .map_err(fail)?;
    expect(
        "fold([[1, 2, 3], [10]])",
        folded.as_ref(),
        &[Val::List(Box::new([
            Val::U32(6),
            Val::U32(10),
            Val::U32(16),
        ]))],
    )?;

    // Both arms of a `result`, one of them carrying a variant.
    let point = |x: i32, y: i32| {
        Val::Record(Box::new([
            ValField {
                name: "x".to_owned(),
                value: Val::S32(x),
            },
            ValField {
                name: "y".to_owned(),
                value: Val::S32(y),
            },
        ]))
    };
    let measured = call("measure-all")?
        .call(
            &mut store,
            &[Val::List(Box::new([
                Val::Variant {
                    discriminant: "dot".to_owned(),
                    payload: Some(Box::new(point(3, -4))),
                },
                Val::Variant {
                    discriminant: "empty".to_owned(),
                    payload: None,
                },
            ]))],
        )
        .await
        .map_err(fail)?;
    expect(
        "measure-all([dot(3, -4), empty])",
        measured.as_ref(),
        &[Val::List(Box::new([
            Val::Result(Ok(Some(Box::new(point(3, -4))))),
            Val::Result(Err(Some(Box::new(Val::Variant {
                discriminant: "blank".to_owned(),
                payload: None,
            })))),
        ]))],
    )?;

    // The support component's resource, constructed and dropped by
    // the guest, and the guest's own resource, constructed and
    // dropped by the driver. Each destructor runs in the component
    // that defines it, and each count is read back through it.
    let steps =
        |values: &[u32]| Val::List(values.iter().copied().map(Val::U32).collect::<Box<[_]>>());
    let tallied = call("exercise-tallies")?
        .call(&mut store, &[steps(&[1, 2, 3])])
        .await
        .map_err(fail)?;
    expect(
        "exercise-tallies([1, 2, 3])",
        tallied.as_ref(),
        &[Val::U32(106)],
    )?;
    let tally_drops = call("tally-drops")?
        .call(&mut store, &[])
        .await
        .map_err(fail)?;
    expect("tally-drops", tally_drops.as_ref(), &[Val::U32(2)])?;

    let counted = call("exercise-counters")?
        .call(&mut store, &[steps(&[5, 7, 9])])
        .await
        .map_err(fail)?;
    expect(
        "exercise-counters([5, 7, 9])",
        counted.as_ref(),
        &[Val::U32(28)],
    )?;
    let counter_drops = call("counter-drops")?
        .call(&mut store, &[])
        .await
        .map_err(fail)?;
    expect("counter-drops", counter_drops.as_ref(), &[Val::U32(2)])?;

    Ok("every shape crossed three component boundaries; both \
        destructors ran twice"
        .to_owned())
}

/// The interface the `wasi-http` handler imports its request and
/// response types from, which only a host supplies.
const WASI_HTTP_TYPES: &str = "wasi:http/types@0.3.0";

/// The `wasi-http` fixture holds a target rather than a result: the
/// polyfill translates the handler, whose request and response carry
/// streams and futures and whose binding layer links the cancel
/// built-ins, but this step supplies no host for `wasi:http/types`.
/// So the step asserts that instantiation stops at link and names
/// that import, as `tests/corpus/expected-failures.txt` does for the
/// fixture's directives. Matching the import rather than any error
/// keeps an unrelated decode or translation bug from passing as the
/// expected failure. Once a host supplies the import through the
/// host surface, this step takes the call the fixture's assertions
/// describe.
async fn wasi_http(engine: &Engine) -> Result<String, String> {
    let component = Component::new(engine, WASI_HTTP)
        .await
        .map_err(|error| format!("the polyfill no longer translates the handler: {error}"))?;
    let linker: Linker<HostState> = Linker::new(engine);
    let mut store = Store::new(engine, HostState::default()).map_err(fail)?;
    match linker.instantiate(&mut store, &component).await {
        Ok(_) => Err(format!(
            "the handler instantiated without a host for {WASI_HTTP_TYPES}: give this \
             step the call `fixtures/wasi-http/assertions.wast` describes, and take the \
             fixture off the expected-failure list"
        )),
        Err(Error::Link(link))
            if matches!(
                &*link,
                LinkError::UnresolvedImport { import, item: None }
                    if import.to_string() == WASI_HTTP_TYPES
            ) =>
        {
            Ok(format!(
                "the polyfill translates the handler, and instantiating it with no \
                 host for {WASI_HTTP_TYPES} stops at link and names that import, as \
                 the fixture's expected failure records"
            ))
        }
        Err(other) => Err(format!(
            "instantiating the handler failed, but not at link for {WASI_HTTP_TYPES} \
             as `tests/corpus/expected-failures.txt` records: {other}"
        )),
    }
}
