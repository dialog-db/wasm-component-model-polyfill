//! Baseline tests for the gates a guest's pointer passes before the
//! host reads through it or reserves anything behind it.
//!
//! Three pointers reach the polyfill from the guest: the pointer and
//! length of a list, the return pointer of a result too wide for
//! flat slots, and the address of a parameter tuple too wide for
//! flat slots. Each is checked for alignment and bounds before a
//! single element is read, and a list's length is checked before a
//! single element's worth of capacity is reserved — a guest that
//! presents `0xFFFF_FFFF` elements would otherwise have the host ask
//! the allocator for about 160 GiB natively, and overflow the
//! capacity computation in the browser, where a pointer is 32 bits
//! wide.
//!
//! A list's pointer and length reach that gate two ways — in a pair
//! of flat slots, and out of a pair of fields in memory — and the
//! gate is one shared helper, so a test here drives each caller.
//!
//! The wording of each trap is Wasmtime's, read at the corpus commit
//! `cb091c33cece` that `corpus/README.md` records:
//! `crates/wasmtime/src/runtime/component/values.rs` for the list
//! pointer (`load_list`), `func/typed.rs` for the return pointer
//! (`lift_heap_result`), and `func/host.rs` for the spilled
//! parameter tuple (`validate_inbounds_dynamic`).
//!
//! The file installs a counting global allocator, so the test of the
//! ungated length can assert what it is really about: that nothing
//! of that size was ever asked for.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use wasm_component_model_polyfill::{
    Component, Engine, Error, FunctionParameter, FunctionType, HostCall, Instance, Linker,
    ListType, PrimitiveType, Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The size at or above which an allocation is one no crossing of
/// these tests has any business making. A list of `0xFFFF_FFFF`
/// elements is far larger than this whatever an element weighs, and
/// nothing else these tests do comes near it.
const OUTSIZED: usize = 1 << 30;

/// How many outsized allocations have been asked for since the test
/// binary started.
static OUTSIZED_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// A global allocator that counts the allocations no lift should
/// ever ask for and passes everything through to the system's.
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

/// The whole error chain of `error`, which is where a failure raised
/// inside a host trampoline surfaces: the guest's call traps, and the
/// cause the trampoline raised is the trap's source.
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

/// A guest that hands the host a list it never wrote: one of
/// `0xFFFF_FFFF` bytes, and one of `u32` elements at an address no
/// `u32` may sit at. The memory lives in its own core module so the
/// lowering can name it, and the calling module needs none of its
/// own — every pointer it passes is a constant.
const LISTS: &[u8] = component!(
    r#"
    (component
      (import "bytes" (func $bytes (param "xs" (list u8))))
      (import "words" (func $words (param "xs" (list u32))))
      (core module $mem (memory (export "memory") 1))
      (core instance $mi (instantiate $mem))
      (core func $bytes-lower
        (canon lower (func $bytes) (memory (core memory $mi "memory"))))
      (core func $words-lower
        (canon lower (func $words) (memory (core memory $mi "memory"))))
      (core module $m
        (import "" "bytes" (func $bytes (param i32 i32)))
        (import "" "words" (func $words (param i32 i32)))
        (func (export "huge") (call $bytes (i32.const 0) (i32.const 0xFFFFFFFF)))
        (func (export "misaligned") (call $words (i32.const 1) (i32.const 1))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "bytes" (func $bytes-lower))
          (export "words" (func $words-lower))))))
      (func (export "huge") (canon lift (core func $i "huge")))
      (func (export "misaligned") (canon lift (core func $i "misaligned"))))
    "#
);

/// Instantiate [`LISTS`] with the two host functions its imports
/// name. Neither body is ever reached: the list each is called with
/// fails the gate while the arguments are still being lifted.
async fn lists() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LISTS)
        .await
        .expect("a component that hands lists to the host parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    for (name, element) in [("bytes", PrimitiveType::U8), ("words", PrimitiveType::U32)] {
        linker.root().func_new(
            name,
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "xs".to_owned(),
                    ty: ValueType::List(ListType::new(ValueType::Primitive(element))),
                }],
                result: None,
                async_: false,
            },
            |_: HostCall<'_, ()>, _args, _results| {
                panic!("the host is never reached: the list fails the gate first")
            },
        );
    }
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_bounds_a_flat_list_before_it_reserves_anything() {
    // The parameters of this import fit in flat slots, so the list
    // arrives as a pointer slot and a length slot rather than as a
    // pair of fields in memory. The length is the whole of a `u32`,
    // which no memory of any size holds.
    let (mut store, instance) = lists().await;
    let before = OUTSIZED_ALLOCATIONS.load(Ordering::Relaxed);
    let err = instance
        .get_func("huge")
        .expect("`huge` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("a length of `0xFFFF_FFFF` leaves the memory");
    assert!(
        chain(&err).contains("list pointer/length out of bounds of memory"),
        "expected Wasmtime's out-of-bounds wording, got {err:?}"
    );
    // The gate runs before the elements are reserved, so the
    // allocator was never asked for the length the guest named.
    // Natively that request is about 160 GiB, which aborts the
    // process; in the browser it overflows the capacity computation
    // and panics the page, so reaching this line at all is the other
    // half of the proof.
    assert_eq!(
        OUTSIZED_ALLOCATIONS.load(Ordering::Relaxed),
        before,
        "nothing of that size was allocated before the bounds check"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_misaligned_list_pointer() {
    // A `list<u32>` is read four bytes at a time, so its pointer is
    // aligned to four. The reference traps on the alignment before
    // it reads, and so does Wasmtime's `load_list`.
    let (mut store, instance) = lists().await;
    let err = instance
        .get_func("misaligned")
        .expect("`misaligned` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("an odd address is no place for a `u32` element");
    assert!(
        chain(&err).contains("list pointer is not aligned"),
        "expected Wasmtime's list-alignment wording, got {err:?}"
    );
}

/// A guest whose exported function returns a `list<u32>`. A list
/// takes two flat slots and a result may take one, so the list
/// travels through a return pointer: the host reads the list's own
/// pointer and length out of a pair of fields in memory rather than
/// out of a pair of flat slots. Both exports write those fields
/// themselves — one naming an address a `u32` may sit at, one
/// naming an odd address.
const LIST_IN_MEMORY: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "good") (result i32)
          (i32.store (i32.const 16) (i32.const 7))
          (i32.store (i32.const 20) (i32.const 11))
          (i32.store (i32.const 8) (i32.const 16))
          (i32.store (i32.const 12) (i32.const 2))
          (i32.const 8))
        (func (export "misaligned") (result i32)
          (i32.store (i32.const 8) (i32.const 17))
          (i32.store (i32.const 12) (i32.const 1))
          (i32.const 8)))
      (core instance $i (instantiate $m))
      (func (export "good") (result (list u32))
        (canon lift (core func $i "good") (memory (core memory $i "memory"))))
      (func (export "misaligned") (result (list u32))
        (canon lift (core func $i "misaligned") (memory (core memory $i "memory")))))
    "#
);

/// Instantiate [`LIST_IN_MEMORY`], which imports nothing.
async fn list_in_memory() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LIST_IN_MEMORY)
        .await
        .expect("a component whose result is a list parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_refuses_a_misaligned_list_pointer_read_out_of_memory() {
    // The gate is one helper, and this is its other caller: the
    // pointer and the length come from a pair of fields at the
    // return pointer, not from a pair of flat slots. The good case
    // below reads its two words through the same fields, so the
    // refusal that follows is the alignment alone.
    let (mut store, instance) = list_in_memory().await;
    let ok = instance
        .get_func("good")
        .expect("`good` is exported")
        .call(&mut store, &[])
        .await
        .expect("an aligned list inside the page reads");
    assert_eq!(
        ok.as_ref(),
        &[Val::List(Box::new([Val::U32(7), Val::U32(11)]))]
    );

    let err = instance
        .get_func("misaligned")
        .expect("`misaligned` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("an odd address is no place for a `u32` element");
    assert!(
        chain(&err).contains("list pointer is not aligned"),
        "expected Wasmtime's list-alignment wording, got {err:?}"
    );
}

/// A guest whose result is a `tuple<u32, u32>` — two flat slots,
/// one more than a result may take — so every export here returns
/// the address of the tuple instead. Each returns an address the
/// host must refuse: an odd one, and one whose tuple would run off
/// the end of the single page the module owns.
const RETURN_POINTERS: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 1)
        (func (export "aligned") (result i32) (i32.const 8))
        (func (export "misaligned") (result i32) (i32.const 3))
        (func (export "far") (result i32) (i32.const 0xFFFC)))
      (core instance $i (instantiate $m))
      (func (export "aligned") (result (tuple u32 u32))
        (canon lift (core func $i "aligned") (memory (core memory $i "memory"))))
      (func (export "misaligned") (result (tuple u32 u32))
        (canon lift (core func $i "misaligned") (memory (core memory $i "memory"))))
      (func (export "far") (result (tuple u32 u32))
        (canon lift (core func $i "far") (memory (core memory $i "memory")))))
    "#
);

/// Instantiate [`RETURN_POINTERS`], which imports nothing.
async fn return_pointers() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, RETURN_POINTERS)
        .await
        .expect("a component with a spilled result parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_refuses_a_misaligned_return_pointer() {
    // The same export at an aligned address reads the two words the
    // page holds, so the refusal below is the alignment and nothing
    // else about the shape of the call.
    let (mut store, instance) = return_pointers().await;
    let ok = instance
        .get_func("aligned")
        .expect("`aligned` is exported")
        .call(&mut store, &[])
        .await
        .expect("an aligned return pointer inside the page reads");
    assert_eq!(
        ok.as_ref(),
        &[Val::Tuple(Box::new([Val::U32(0), Val::U32(0)]))]
    );

    let err = instance
        .get_func("misaligned")
        .expect("`misaligned` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("an odd address is no place for a tuple of words");
    assert!(
        chain(&err).contains("return pointer not aligned"),
        "expected Wasmtime's return-pointer wording, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_bounds_a_return_pointer_against_the_memory() {
    // Four bytes short of the end of a one-page memory is an aligned
    // address, and the eight-byte tuple that starts there is not
    // inside the page.
    let (mut store, instance) = return_pointers().await;
    let err = instance
        .get_func("far")
        .expect("`far` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("the tuple runs off the end of the page");
    assert!(
        chain(&err).contains("pointer out of bounds of memory"),
        "expected Wasmtime's return-bounds wording, got {err:?}"
    );
}

/// A guest that calls a host import of seventeen `u32` parameters —
/// one flat slot more than a parameter tuple may take, so the whole
/// tuple travels through one address. Each export passes an address
/// the host must refuse.
const SPILLED_ARGUMENTS: &[u8] = component!(
    r#"
    (component
      (import "wide" (func $wide
        (param "p0" u32) (param "p1" u32) (param "p2" u32) (param "p3" u32)
        (param "p4" u32) (param "p5" u32) (param "p6" u32) (param "p7" u32)
        (param "p8" u32) (param "p9" u32) (param "p10" u32) (param "p11" u32)
        (param "p12" u32) (param "p13" u32) (param "p14" u32) (param "p15" u32)
        (param "p16" u32)))
      (core module $mem (memory (export "memory") 1))
      (core instance $mi (instantiate $mem))
      (core func $wide-lower
        (canon lower (func $wide) (memory (core memory $mi "memory"))))
      (core module $m
        (import "" "wide" (func $wide (param i32)))
        (func (export "aligned") (call $wide (i32.const 8)))
        (func (export "misaligned") (call $wide (i32.const 2)))
        (func (export "far") (call $wide (i32.const 0xFFF0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "wide" (func $wide-lower))))))
      (func (export "aligned") (canon lift (core func $i "aligned")))
      (func (export "misaligned") (canon lift (core func $i "misaligned")))
      (func (export "far") (canon lift (core func $i "far"))))
    "#
);

/// Instantiate [`SPILLED_ARGUMENTS`] with the wide host function its
/// import names. The body counts the calls that reach it, so a test
/// can say both that a good address arrives and that a bad one never
/// does.
async fn spilled_arguments(reached: std::sync::Arc<AtomicUsize>) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, SPILLED_ARGUMENTS)
        .await
        .expect("a component with a spilled parameter tuple parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().func_new(
        "wide",
        FunctionType {
            parameters: (0..17)
                .map(|i| FunctionParameter {
                    name: format!("p{i}"),
                    ty: ValueType::Primitive(PrimitiveType::U32),
                })
                .collect(),
            result: None,
            async_: false,
        },
        move |_: HostCall<'_, ()>, args, _results| {
            assert_eq!(args.len(), 17, "every parameter of the tuple is lifted");
            reached.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
    );
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_refuses_a_misaligned_spilled_argument_tuple() {
    let reached = std::sync::Arc::new(AtomicUsize::new(0));
    let (mut store, instance) = spilled_arguments(reached.clone()).await;

    // The same tuple at an aligned address inside the page lifts, so
    // the refusal below is the alignment alone.
    instance
        .get_func("aligned")
        .expect("`aligned` is exported")
        .call(&mut store, &[])
        .await
        .expect("an aligned tuple inside the page lifts");
    assert_eq!(reached.load(Ordering::Relaxed), 1);

    let err = instance
        .get_func("misaligned")
        .expect("`misaligned` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("a tuple of words does not start two bytes in");
    assert!(
        chain(&err).contains("pointer not aligned"),
        "expected Wasmtime's tuple-alignment wording, got {err:?}"
    );
    assert_eq!(
        reached.load(Ordering::Relaxed),
        1,
        "the host was not reached with a half-lifted tuple"
    );
}

#[wcmp_macros::test]
async fn it_bounds_a_spilled_argument_tuple_as_a_whole() {
    // Sixteen bytes short of the end of the page is where four of
    // the seventeen words fit and thirteen do not. The tuple is
    // measured as one thing, so the call fails before the first
    // parameter is lifted rather than part-way through.
    let reached = std::sync::Arc::new(AtomicUsize::new(0));
    let (mut store, instance) = spilled_arguments(reached.clone()).await;
    let err = instance
        .get_func("far")
        .expect("`far` is exported")
        .call(&mut store, &[])
        .await
        .expect_err("the tuple runs off the end of the page");
    assert!(
        chain(&err).contains("pointer out of bounds"),
        "expected Wasmtime's tuple-bounds wording, got {err:?}"
    );
    assert_eq!(
        reached.load(Ordering::Relaxed),
        0,
        "the host was not reached with a half-lifted tuple"
    );
}
