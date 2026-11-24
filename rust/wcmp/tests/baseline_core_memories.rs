// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the memories of the core instances of a
//! component.
//!
//! A host reads the current size of each memory that a core instance
//! of a component defines and exports, through
//! `Instance::core_memories`. It reads the size as it reads the size of
//! a `wasmtime::Memory`: in pages through `size`, and in bytes through
//! `data_size`. A host that bounds the memory of a guest reads the size
//! when the guest traps, to tell whether the guest had grown its memory
//! to the bound. So a read works in a store that a trap poisoned.

#![cfg(test)]

use wcmp::{
    Component, Engine, Error, Instance, InstantiationError, Linker, Module, Store, TaskCause,
};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The size of a page of a memory, in bytes.
const PAGE: usize = 65_536;

/// A component whose core instances hold memories of each kind the
/// list of memories tells apart.
///
/// - `$defines` defines and exports a memory of 1 page that can grow to
///   4. Its exports grow it: `grow` by the pages it is given, and
///   `exhaust` until a grow fails, and then it traps, as a Rust guest
///   aborts when its allocator gets no more memory. `ok` answers 7.
/// - `$reexports` imports the memory of `$defines` and exports it again.
/// - `$hidden` defines a memory of 2 pages and does not export it.
/// - `$first` and `$second` are two instances of one module, which
///   exports its memory of 3 pages under two names.
/// - A nested component defines and exports a memory of 5 pages.
const MEMORIES: &[u8] = component!(
    r#"
    (component
      (core module $Defines
        (memory (export "memory") 1 4)
        (func (export "grow") (param i32) (result i32)
          (memory.grow (local.get 0)))
        (func (export "exhaust")
          (block $full
            (loop $more
              (br_if $full (i32.eq (memory.grow (i32.const 1)) (i32.const -1)))
              (br $more)))
          unreachable)
        (func (export "ok") (result i32) (i32.const 7)))
      (core module $Reexports
        (import "" "memory" (memory 1))
        (export "memory" (memory 0)))
      (core module $Hidden
        (memory 2))
      (core module $TwoNames
        (memory (export "a") 3)
        (export "b" (memory 0)))
      (core instance $defines (instantiate $Defines))
      (core instance $reexports (instantiate $Reexports (with "" (instance $defines))))
      (core instance $hidden (instantiate $Hidden))
      (core instance $first (instantiate $TwoNames))
      (core instance $second (instantiate $TwoNames))
      (component $Inner
        (core module $M
          (memory (export "memory") 5))
        (core instance (instantiate $M)))
      (instance (instantiate $Inner))
      (func (export "grow") (param "pages" u32) (result s32)
        (canon lift (core func $defines "grow")))
      (func (export "exhaust")
        (canon lift (core func $defines "exhaust")))
      (func (export "ok") (result u32)
        (canon lift (core func $defines "ok"))))
    "#
);

/// A component that instantiates a core module the host supplies.
const IMPORTS_A_MODULE: &[u8] = component!(
    r#"
    (component
      (import "m" (core module $M
        (export "memory" (memory 1))))
      (core instance (instantiate $M)))
    "#
);

/// A core module that defines and exports a memory of 6 pages.
const MEMORY_OF_SIX_PAGES: &[u8] = wasm!(
    r#"
    (module
      (memory (export "memory") 6))
    "#
);

/// An engine, a store, and an instance of [`MEMORIES`] in the store.
struct Fixture {
    engine: Engine,
    store: Store<()>,
    instance: Instance,
}

impl Fixture {
    async fn new() -> Self {
        let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
        let component = Component::new(&engine, MEMORIES)
            .await
            .expect("the component parses");
        let linker: Linker<()> = Linker::new(&engine);
        let mut store = Store::new(&engine, ()).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("the component instantiates");
        Self {
            engine,
            store,
            instance,
        }
    }

    /// The size of each memory of the instance, in pages.
    fn pages(&self) -> Vec<u64> {
        self.instance
            .core_memories()
            .iter()
            .map(|memory| memory.size(&self.store).expect("the size of the memory"))
            .collect()
    }

    /// The size of each memory of the instance, in bytes.
    fn bytes(&self) -> Vec<usize> {
        self.instance
            .core_memories()
            .iter()
            .map(|memory| {
                memory
                    .data_size(&self.store)
                    .expect("the size of the memory")
            })
            .collect()
    }
}

#[wcmp_macros::test]
async fn it_lists_each_memory_that_a_core_instance_defines_and_exports() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.pages(),
        [1, 3, 3, 5],
        "the memory of `$defines`, one of each instance of `$TwoNames`, and the memory of the \
         nested component, with neither the memory `$reexports` exports again nor the memory \
         `$hidden` keeps"
    );
    assert_eq!(fixture.bytes(), [PAGE, 3 * PAGE, 3 * PAGE, 5 * PAGE]);
}

#[wcmp_macros::test]
async fn it_reads_the_size_that_a_memory_grows_to() {
    let mut fixture = Fixture::new().await;
    let grow = fixture
        .instance
        .get_func("grow")
        .expect("`grow` is exported")
        .typed::<(u32,), (i32,)>()
        .expect("`grow` takes a u32 and gives an s32");
    let (old,) = grow
        .call(&mut fixture.store, (2,))
        .await
        .expect("the memory grows");
    assert_eq!(old, 1, "the memory grew from 1 page");
    assert_eq!(fixture.pages(), [3, 3, 3, 5]);
    assert_eq!(fixture.bytes(), [3 * PAGE, 3 * PAGE, 3 * PAGE, 5 * PAGE]);
}

#[wcmp_macros::test]
async fn it_reads_the_size_of_a_memory_after_a_trap() {
    let mut fixture = Fixture::new().await;
    let exhaust = fixture
        .instance
        .get_func("exhaust")
        .expect("`exhaust` is exported")
        .typed::<(), ()>()
        .expect("`exhaust` takes nothing and gives nothing");
    let error = exhaust
        .call(&mut fixture.store, ())
        .await
        .expect_err("the guest traps when its memory cannot grow");
    assert!(
        matches!(error, Error::Trap(_)),
        "the guest traps, got {error:?}"
    );
    let ok = fixture
        .instance
        .get_func("ok")
        .expect("`ok` is exported")
        .typed::<(), (u32,)>()
        .expect("`ok` gives a u32");
    let error = ok
        .call(&mut fixture.store, ())
        .await
        .expect_err("the trap poisoned the store");
    assert!(
        matches!(error, Error::Task(TaskCause::CannotEnter)),
        "a call after the trap fails with the cannot-enter cause, got {error:?}"
    );
    assert_eq!(
        fixture.pages(),
        [4, 3, 3, 5],
        "the memory of `$defines` is at its maximum of 4 pages"
    );
    assert_eq!(fixture.bytes(), [4 * PAGE, 3 * PAGE, 3 * PAGE, 5 * PAGE]);
}

#[wcmp_macros::test]
async fn it_lists_the_memory_of_a_core_module_that_the_host_supplies() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, IMPORTS_A_MODULE)
        .await
        .expect("the component parses");
    let module = Module::new(&engine, MEMORY_OF_SIX_PAGES)
        .await
        .expect("the module compiles");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .module("m", &module)
        .expect("the registration of `m`");
    let mut store = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    let sizes = instance
        .core_memories()
        .iter()
        .map(|memory| memory.size(&store).expect("the size of the memory"))
        .collect::<Vec<_>>();
    assert_eq!(sizes, [6]);
}

#[wcmp_macros::test]
async fn it_refuses_a_read_through_another_store() {
    let fixture = Fixture::new().await;
    let other = Store::new(&fixture.engine, ()).expect("store");
    let memory = fixture
        .instance
        .core_memories()
        .first()
        .copied()
        .expect("the instance has a memory");
    for (read, error) in [
        ("size", memory.size(&other).map(|_| ()).unwrap_err()),
        (
            "data_size",
            memory.data_size(&other).map(|_| ()).unwrap_err(),
        ),
    ] {
        assert!(
            matches!(&error, Error::Instantiation(inner) if matches!(**inner, InstantiationError::WrongStore)),
            "`{read}` through another store fails with the wrong-store cause, got {error:?}"
        );
    }
}

#[path = "support/backend.rs"]
mod test_backend;
