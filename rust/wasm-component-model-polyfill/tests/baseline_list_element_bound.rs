//! Baseline tests for the bound on the list elements one crossing
//! may lift out of a guest.
//!
//! The host holds one `Val` per element of a lifted list, which is
//! several times what the element occupies in guest memory: a
//! `list<u8>` whose byte range a large memory holds can still ask the
//! host for many times that memory. The engine configuration bounds
//! the elements one crossing may lift, counted over every list of the
//! crossing, and the list that would pass the bound is refused with a
//! structured cause before anything is reserved for it.
//!
//! The file installs a counting global allocator, so the test of the
//! default bound can assert what it is really about: that the
//! reservation the refused list would have made was never asked for.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_component_model_polyfill::{
    AbiCause, Component, Engine, EngineConfig, Error, FunctionParameter, FunctionType, Instance,
    InterfaceIdentifier, Linker, ListType, PrimitiveType, Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The size at or above which an allocation is the refused list's
/// reservation. The list is one element past the default bound of
/// 4 194 304, and a `Val` is well over eight bytes on either target,
/// so its elements alone would be more than this; the guest's own
/// memory and the one copy of its bytes the lift reads are each
/// about four MiB, well under it.
const OUTSIZED: usize = 1 << 25;

/// How many outsized allocations have been asked for since the test
/// binary started.
static OUTSIZED_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// A global allocator that counts the allocations the refused list
/// would have made and passes everything through to the system's.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= OUTSIZED {
            OUTSIZED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= OUTSIZED {
            OUTSIZED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= OUTSIZED {
            OUTSIZED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The default bound, as `EngineConfig::max_list_elements` documents
/// it.
const DEFAULT_BOUND: usize = 1 << 22;

/// A guest that returns lists of whatever length the host asks for,
/// all over the same bytes: `one` returns one list of `n` bytes, `two`
/// a pair of them, and `nested` a list of two of them. Its memory is
/// 65 pages, a little over four MiB, so a list one element past the
/// default bound lies inside it and passes every check on its byte
/// range.
const LISTS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 65)
        (func (export "one") (param $n i32) (result i32)
          (i32.store (i32.const 0) (i32.const 16))
          (i32.store (i32.const 4) (local.get $n))
          i32.const 0)
        (func (export "two") (param $n i32) (result i32)
          (i32.store (i32.const 0) (i32.const 16))
          (i32.store (i32.const 4) (local.get $n))
          (i32.store (i32.const 8) (i32.const 16))
          (i32.store (i32.const 12) (local.get $n))
          i32.const 0)
        (func (export "nested") (param $n i32) (result i32)
          (i32.store (i32.const 0) (i32.const 16))
          (i32.store (i32.const 4) (i32.const 2))
          (i32.store (i32.const 16) (i32.const 32))
          (i32.store (i32.const 20) (local.get $n))
          (i32.store (i32.const 24) (i32.const 32))
          (i32.store (i32.const 28) (local.get $n))
          i32.const 0))
      (core instance $i (instantiate $m))
      (func (export "one") (param "n" u32) (result (list u8))
        (canon lift (core func $i "one") (memory (core memory $i "memory"))))
      (func (export "two") (param "n" u32) (result (tuple (list u8) (list u8)))
        (canon lift (core func $i "two") (memory (core memory $i "memory"))))
      (func (export "nested") (param "n" u32) (result (list (list u8)))
        (canon lift (core func $i "nested") (memory (core memory $i "memory")))))
    "#
);

/// Instantiate [`LISTS`] against an engine built from `config`.
async fn lists(config: &EngineConfig) -> (Store<()>, Instance) {
    let engine = Engine::with_config(config).expect("engine");
    let component = Component::new(&engine, LISTS)
        .await
        .expect("a component that returns lists parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

/// An engine configuration whose bound is `limit` elements.
fn bounded(limit: usize) -> EngineConfig {
    let mut config = EngineConfig::new();
    config.max_list_elements(limit);
    config
}

/// Call `name` with `n`.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    n: usize,
) -> Result<Box<[Val]>, Error> {
    let n = u32::try_from(n).expect("a length a guest can name");
    instance
        .get_func(name)
        .expect("the export")
        .call(store, &[Val::U32(n)])
        .await
}

/// The structured cause a refused list fails with, as its three
/// numbers: the list's length, the elements the crossing had left,
/// and the bound.
fn element_limit(error: &Error) -> (usize, usize, usize) {
    match error {
        Error::Abi(abi) => match abi.cause {
            AbiCause::ListElementLimit {
                length,
                remaining,
                limit,
            } => (length, remaining, limit),
            ref other => panic!("expected the element-limit cause, got {other:?}"),
        },
        other => panic!("expected an ABI failure, got {other:?}"),
    }
}

/// The lengths of the lists in a lifted result: one list, or a tuple
/// of them.
fn lengths(value: &Val) -> Vec<usize> {
    match value {
        Val::List(elements) => vec![elements.len()],
        Val::Tuple(items) => items.iter().flat_map(lengths).collect(),
        other => panic!("expected a list or a tuple of lists, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_refuses_a_list_past_the_default_bound_before_reserving_it() {
    // The list's bytes lie inside the guest's memory, so every check
    // on its byte range passes; only its element count is too many.
    let (mut store, instance) = lists(&EngineConfig::new()).await;
    let before = OUTSIZED_ALLOCATIONS.load(Ordering::Relaxed);
    let error = call(&mut store, &instance, "one", DEFAULT_BOUND + 1)
        .await
        .expect_err("a list one element past the default bound is refused");
    assert_eq!(
        element_limit(&error),
        (DEFAULT_BOUND + 1, DEFAULT_BOUND, DEFAULT_BOUND)
    );
    assert_eq!(
        OUTSIZED_ALLOCATIONS.load(Ordering::Relaxed),
        before,
        "nothing was reserved for the refused list's elements"
    );
}

#[wcmp_macros::test]
async fn it_lifts_a_list_as_long_as_the_configured_bound() {
    let (mut store, instance) = lists(&bounded(16)).await;
    let results = call(&mut store, &instance, "one", 16)
        .await
        .expect("a list of exactly the bound lifts");
    assert_eq!(lengths(&results[0]), [16]);
}

#[wcmp_macros::test]
async fn it_refuses_a_list_one_element_past_the_configured_bound() {
    let (mut store, instance) = lists(&bounded(16)).await;
    let error = call(&mut store, &instance, "one", 17)
        .await
        .expect_err("a list one element past the bound is refused");
    assert_eq!(element_limit(&error), (17, 16, 16));
}

#[wcmp_macros::test]
async fn it_counts_every_list_of_a_crossing_against_the_bound() {
    // Two lists of eight fill a bound of sixteen between them, and
    // two of nine pass it on the second list, which finds seven
    // elements left.
    let (mut store, instance) = lists(&bounded(16)).await;
    let results = call(&mut store, &instance, "two", 8)
        .await
        .expect("two lists of eight fit a bound of sixteen");
    assert_eq!(lengths(&results[0]), [8, 8]);

    let error = call(&mut store, &instance, "two", 9)
        .await
        .expect_err("two lists of nine pass a bound of sixteen");
    assert_eq!(element_limit(&error), (9, 7, 16));
}

#[wcmp_macros::test]
async fn it_gives_every_crossing_the_whole_bound() {
    // The count belongs to one crossing: a call after one that used
    // the whole bound starts from the whole bound again.
    let (mut store, instance) = lists(&bounded(16)).await;
    for _ in 0..3 {
        let results = call(&mut store, &instance, "one", 16)
            .await
            .expect("each call lifts a list of the whole bound");
        assert_eq!(lengths(&results[0]), [16]);
    }
}

#[wcmp_macros::test]
async fn it_counts_a_nested_list_and_its_elements_against_the_bound() {
    // The outer list's two elements count too: two inner lists of
    // seven fill a bound of sixteen with it, and two of eight pass it
    // on the second inner list, which finds six elements left.
    let (mut store, instance) = lists(&bounded(16)).await;
    let results = call(&mut store, &instance, "nested", 7)
        .await
        .expect("a list of two lists of seven fits a bound of sixteen");
    let Val::List(inner) = &results[0] else {
        panic!("expected a list, got {:?}", results[0]);
    };
    let inner: Vec<usize> = inner.iter().flat_map(lengths).collect();
    assert_eq!(inner, [7, 7]);

    let error = call(&mut store, &instance, "nested", 8)
        .await
        .expect_err("a list of two lists of eight passes a bound of sixteen");
    assert_eq!(element_limit(&error), (8, 6, 16));
}

/// A guest that hands a host import two lists of `n` bytes as its
/// arguments, both over the same bytes of its memory.
const SENDER: &[u8] = component!(
    r#"
    (component
      (type $iface (instance
        (export "take" (func (param "a" (list u8)) (param "b" (list u8))))))
      (import "wcmp-tests:host/bytes@0.1.0" (instance $imports (type $iface)))
      (alias export $imports "take" (func $take))
      (core module $libc
        (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $core-take
        (canon lower (func $take) (memory (core memory $libc "memory"))))
      (core module $m
        (import "host" "take" (func $take (param i32 i32 i32 i32)))
        (func (export "send") (param $n i32)
          i32.const 16
          local.get $n
          i32.const 16
          local.get $n
          call $take))
      (core instance $i (instantiate $m
        (with "host" (instance (export "take" (func $core-take))))))
      (func (export "send") (param "n" u32)
        (canon lift (core func $i "send"))))
    "#
);

#[wcmp_macros::test]
async fn it_counts_the_lists_a_guest_passes_a_host_import_against_the_bound() {
    let engine = Engine::with_config(&bounded(16)).expect("engine");
    let component = Component::new(&engine, SENDER)
        .await
        .expect("a component that sends lists parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let iface: InterfaceIdentifier = "wcmp-tests:host/bytes@0.1.0".parse().expect("identifier");
    let bytes = || ValueType::List(ListType::new(ValueType::Primitive(PrimitiveType::U8)));
    let signature = FunctionType {
        parameters: vec![
            FunctionParameter {
                name: "a".to_owned(),
                ty: bytes(),
            },
            FunctionParameter {
                name: "b".to_owned(),
                ty: bytes(),
            },
        ],
        result: None,
        async_: false,
    };
    linker
        .instance(&iface)
        .func_new("take", signature, |_, args, _| {
            let received: Vec<usize> = args.iter().flat_map(lengths).collect();
            assert_eq!(received, [8, 8], "only lists inside the bound arrive");
            Ok(())
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");

    call(&mut store, &instance, "send", 8)
        .await
        .expect("two lists of eight fit a bound of sixteen");
    // The refusal reaches the guest as a trap of its call to the
    // import, so it comes back to the host in the trap's words.
    let error = call(&mut store, &instance, "send", 9)
        .await
        .expect_err("two lists of nine pass a bound of sixteen");
    let text = chain(&error);
    assert!(
        text.contains("a list of 9 elements, with 7 of the 16 the crossing may lift left"),
        "expected the element-limit cause, got {text}"
    );
}

/// `error` and every error beneath it, joined into one line.
fn chain(error: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out
}
