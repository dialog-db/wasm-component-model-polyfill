//! Baseline tests for the copy budget every crossing carries.
//!
//! A crossing may build as many bytes of host values out of a guest
//! as the store's hostcall fuel allows, 128 MiB by default as in
//! Wasmtime. Each lift charges it before reserving anything: a list
//! or a fixed-length list 32 bytes per element, a map 64 bytes per
//! entry, a string the byte length of its range, and a typed vector
//! of numbers its own bytes. The costs are fixed, so every test here
//! finds its boundary at the same length natively and in a browser.
//! The lift that would pass the budget fails with a structured cause
//! and Wasmtime's message.
//!
//! The file installs a counting global allocator, so a test can
//! assert that a refused list was refused before the host reserved
//! anything for it. The count is kept per thread and only while a
//! test asks for it: an async test runs its call on the thread it
//! started on, and nothing else in the process is counted.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Arc, Mutex};

use wcmp::{
    AbiCause, Component, Engine, Error, FunctionParameter, FunctionType, Instance,
    InterfaceIdentifier, Linker, ListType, PrimitiveType, Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

thread_local! {
    /// Whether this thread's allocations are being counted.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// The allocations of at least [`OUTSIZED`] bytes this thread has
    /// made while counting.
    static OUTSIZED_COUNT: Cell<usize> = const { Cell::new(0) };
}

/// The size at or above which an allocation is a reservation for a
/// list's elements. A list of a budget's worth of elements holds a
/// `Val` apiece, 96 MiB or more on either target; the guest's memory
/// and the one copy of a list's bytes a lift reads are each well
/// under this.
const OUTSIZED: usize = 1 << 25;

/// Count an allocation of `size` bytes, when this thread is counting.
fn tally(size: usize) {
    if size >= OUTSIZED && COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = OUTSIZED_COUNT.try_with(|count| count.set(count.get() + 1));
    }
}

/// A global allocator that counts the outsized allocations of the
/// thread under measurement and passes everything through to the
/// system's.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Start counting this thread's outsized allocations.
fn start_counting() {
    OUTSIZED_COUNT.with(|count| count.set(0));
    COUNTING.with(|counting| counting.set(true));
}

/// Stop counting and report the outsized allocations since the start.
fn stop_counting() -> usize {
    COUNTING.with(|counting| counting.set(false));
    OUTSIZED_COUNT.with(Cell::get)
}

/// The copy budget a store gives each crossing: 128 MiB, Wasmtime's
/// default hostcall fuel.
const BUDGET: usize = 128 << 20;

/// Wasmtime's message for a spent budget, which a test written
/// against Wasmtime matches by substring.
const MESSAGE: &str = "too much data is being copied between the host and the guest: \
                       fuel allocated for hostcalls has been exhausted";

/// What one element of a list or a fixed-length list costs, on every
/// target.
const ELEMENT: usize = 32;

/// What one entry of a map costs, on every target.
const ENTRY: usize = 64;

/// The bytes of guest memory the values below may range over: 256
/// pages, less the sixteen bytes the return area takes.
const GUEST_BYTES: usize = 256 * 65536 - 16;

/// A guest that returns values of whatever size the host asks for,
/// all over the same zeroed bytes of its memory: `list` returns a
/// list of `n` bytes, `numbers` a list of `n` `u32`s, `map` a map of
/// `n` entries of a `u8` to a `u8`, `string` a string of `n` bytes,
/// `fixed` a fixed-length list of four bytes, `list-and-string` a
/// list of `n` bytes beside a string of `m`, and `nested` a list of
/// two lists of `n` bytes. The values of one result are lifted by
/// one crossing.
const VALUES: &[u8] = component!(
    r#"
    (component
      (core module $m
        (memory (export "memory") 256)
        (func (export "list") (param $n i32) (result i32)
          (i32.store (i32.const 0) (i32.const 16))
          (i32.store (i32.const 4) (local.get $n))
          i32.const 0)
        (func (export "pair") (param $n i32) (param $m i32) (result i32)
          (i32.store (i32.const 0) (i32.const 16))
          (i32.store (i32.const 4) (local.get $n))
          (i32.store (i32.const 8) (i32.const 16))
          (i32.store (i32.const 12) (local.get $m))
          i32.const 0)
        (func (export "fixed") (result i32)
          i32.const 16)
        (func (export "nested") (param $n i32) (result i32)
          (i32.store (i32.const 0) (i32.const 16))
          (i32.store (i32.const 4) (i32.const 2))
          (i32.store (i32.const 16) (i32.const 32))
          (i32.store (i32.const 20) (local.get $n))
          (i32.store (i32.const 24) (i32.const 32))
          (i32.store (i32.const 28) (local.get $n))
          i32.const 0))
      (core instance $i (instantiate $m))
      (func (export "list") (param "n" u32) (result (list u8))
        (canon lift (core func $i "list") (memory (core memory $i "memory"))))
      (func (export "numbers") (param "n" u32) (result (list u32))
        (canon lift (core func $i "list") (memory (core memory $i "memory"))))
      (func (export "map") (param "n" u32) (result (map u8 u8))
        (canon lift (core func $i "list") (memory (core memory $i "memory"))))
      (func (export "string") (param "n" u32) (result string)
        (canon lift (core func $i "list") (memory (core memory $i "memory"))))
      (func (export "fixed") (result (list u8 4))
        (canon lift (core func $i "fixed") (memory (core memory $i "memory"))))
      (func (export "list-and-string") (param "n" u32) (param "m" u32)
        (result (tuple (list u8) string))
        (canon lift (core func $i "pair") (memory (core memory $i "memory"))))
      (func (export "nested") (param "n" u32) (result (list (list u8)))
        (canon lift (core func $i "nested") (memory (core memory $i "memory")))))
    "#
);

/// Instantiate `bytes` into a store of its own, under the default
/// engine configuration, with no imports.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    (store, instance)
}

/// Call `name` with the lengths `args`.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[usize],
) -> Result<Box<[Val]>, Error> {
    let args: Vec<Val> = args
        .iter()
        .map(|&n| Val::U32(u32::try_from(n).expect("a length a guest can name")))
        .collect();
    instance
        .get_func(name)
        .expect("the export")
        .call(store, &args)
        .await
}

/// Assert that `error` is the spent budget, in its structured cause
/// and in Wasmtime's words.
fn assert_budget_spent(error: &Error) {
    match error {
        Error::Abi(abi) => assert!(
            matches!(abi.cause, AbiCause::CopyBudgetSpent),
            "expected the copy-budget cause, got {:?}",
            abi.cause
        ),
        other => panic!("expected an ABI failure, got {other:?}"),
    }
    let text = error.to_string();
    assert!(
        text.contains(MESSAGE),
        "expected Wasmtime's message, got {text}"
    );
}

/// Assert that Wasmtime's message is somewhere in `error`'s chain,
/// which is where a refusal inside a guest's call to the host lands:
/// the guest traps, and the trap carries the refusal.
fn assert_budget_spent_beneath(error: &Error) {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        out.push_str(&link.to_string());
        out.push_str(": ");
        current = link.source();
    }
    assert!(
        out.contains(MESSAGE),
        "expected Wasmtime's message, got {out}"
    );
}

/// The length of the list, string, or map `value` is.
fn length(value: &Val) -> usize {
    match value {
        Val::List(elements) | Val::FixedLengthList(elements) => elements.len(),
        Val::String(text) => text.len(),
        Val::Map(entries) => entries.len(),
        other => panic!("expected a list, a string, or a map, got {other:?}"),
    }
}

/// The lengths of the values of a tuple or a list of them.
fn lengths(value: &Val) -> Vec<usize> {
    match value {
        Val::Tuple(items) | Val::List(items) => items.iter().map(length).collect(),
        other => panic!("expected a tuple or a list, got {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_gives_a_store_wasmtimes_default_hostcall_fuel() {
    let (store, _) = instantiate(VALUES).await;
    assert_eq!(store.hostcall_fuel(), BUDGET);
}

#[wcmp_macros::test]
async fn it_lifts_a_list_result_of_exactly_the_copy_budget_on_every_call() {
    // Each call is a crossing of its own, so each starts from the
    // whole budget. The reservation this list makes is what the next
    // test proves a refused list never makes.
    let (mut store, instance) = instantiate(VALUES).await;
    let n = BUDGET / ELEMENT;
    assert!(n <= GUEST_BYTES, "the list lies inside the guest's memory");
    for _ in 0..2 {
        start_counting();
        let results = call(&mut store, &instance, "list", &[n])
            .await
            .expect("a list of exactly the budget lifts");
        let outsized = stop_counting();
        assert_eq!(length(&results[0]), n);
        assert!(outsized >= 1, "the lifted list reserved its elements");
    }
}

#[wcmp_macros::test]
async fn it_refuses_a_list_result_past_the_copy_budget_before_reserving_it() {
    // The list's bytes lie inside the guest's memory, so every check
    // on its byte range passes; only what it would cost the host is
    // too much.
    let (mut store, instance) = instantiate(VALUES).await;
    let n = BUDGET / ELEMENT + 1;
    assert!(n <= GUEST_BYTES, "the list lies inside the guest's memory");
    start_counting();
    let error = call(&mut store, &instance, "list", &[n])
        .await
        .expect_err("a list of one element past the budget is refused");
    let outsized = stop_counting();
    assert_budget_spent(&error);
    assert_eq!(
        outsized, 0,
        "nothing was reserved for the refused list's elements"
    );
}

#[wcmp_macros::test]
async fn it_charges_a_map_sixty_four_bytes_per_entry() {
    let (mut store, instance) = instantiate(VALUES).await;
    let n = BUDGET / ENTRY;
    assert!(
        2 * n <= GUEST_BYTES,
        "the entries lie inside the guest's memory"
    );
    let results = call(&mut store, &instance, "map", &[n])
        .await
        .expect("a map of exactly the budget lifts");
    assert_eq!(length(&results[0]), n);

    let error = call(&mut store, &instance, "map", &[n + 1])
        .await
        .expect_err("a map of one entry past the budget is refused");
    assert_budget_spent(&error);
}

#[wcmp_macros::test]
async fn it_charges_a_string_the_bytes_of_its_range_under_the_default_budget() {
    // A string of the whole budget is 128 MiB of guest memory, so a
    // list spends all of the crossing's budget but 4 KiB first; a
    // string of exactly that many bytes spends the rest, and one
    // byte more passes it.
    let (mut store, instance) = instantiate(VALUES).await;
    let rest = 4096;
    let n = (BUDGET - rest) / ELEMENT;
    assert_eq!(n * ELEMENT + rest, BUDGET);
    let results = call(&mut store, &instance, "list-and-string", &[n, rest])
        .await
        .expect("a string of the bytes the budget has left lifts");
    assert_eq!(lengths(&results[0]), [n, rest]);

    let error = call(&mut store, &instance, "list-and-string", &[n, rest + 1])
        .await
        .expect_err("a string one byte past the budget is refused");
    assert_budget_spent(&error);
}

#[wcmp_macros::test]
async fn it_gives_each_crossing_the_hostcall_fuel_the_store_holds() {
    // A string alone, against a budget the host lowered: exactly the
    // fuel lifts, a byte more does not, and raising the fuel again
    // lets that string through. The refused lift is a trap, and a trap
    // poisons the store, so the raised fuel is tried in a store of its
    // own.
    let (mut store, instance) = instantiate(VALUES).await;
    store.set_hostcall_fuel(1000);
    assert_eq!(store.hostcall_fuel(), 1000);
    let results = call(&mut store, &instance, "string", &[1000])
        .await
        .expect("a string of exactly the fuel lifts");
    assert_eq!(length(&results[0]), 1000);
    let error = call(&mut store, &instance, "string", &[1001])
        .await
        .expect_err("a string one byte past the fuel is refused");
    assert_budget_spent(&error);

    let (mut store, instance) = instantiate(VALUES).await;
    store.set_hostcall_fuel(1001);
    call(&mut store, &instance, "string", &[1001])
        .await
        .expect("the raised fuel admits the string");
}

#[wcmp_macros::test]
async fn it_charges_a_fixed_length_list_thirty_two_bytes_per_element() {
    let (mut store, instance) = instantiate(VALUES).await;
    store.set_hostcall_fuel(4 * ELEMENT);
    let results = call(&mut store, &instance, "fixed", &[])
        .await
        .expect("four elements lift for their cost");
    assert_eq!(length(&results[0]), 4);

    store.set_hostcall_fuel(4 * ELEMENT - 1);
    let error = call(&mut store, &instance, "fixed", &[])
        .await
        .expect_err("four elements are refused a byte short of their cost");
    assert_budget_spent(&error);
}

#[wcmp_macros::test]
async fn it_charges_every_list_of_a_crossing_nested_ones_included() {
    // The outer list's two elements count too: with sixteen elements'
    // worth of fuel, two inner lists of seven fill it with the outer
    // list, and two of eight pass it.
    let (mut store, instance) = instantiate(VALUES).await;
    store.set_hostcall_fuel(16 * ELEMENT);
    let results = call(&mut store, &instance, "nested", &[7])
        .await
        .expect("a list of two lists of seven fits sixteen elements");
    assert_eq!(lengths(&results[0]), [7, 7]);

    let error = call(&mut store, &instance, "nested", &[8])
        .await
        .expect_err("a list of two lists of eight passes sixteen elements");
    assert_budget_spent(&error);
}

#[wcmp_macros::test]
async fn it_charges_a_typed_vector_of_numbers_its_own_bytes() {
    // A `Vec<u32>` holds four bytes per element and no `Val`, so it
    // is charged four bytes apiece.
    let (mut store, instance) = instantiate(VALUES).await;
    let numbers = instance
        .get_func("numbers")
        .expect("the export")
        .typed::<(u32,), Vec<u32>>()
        .expect("the export has the typed signature");
    store.set_hostcall_fuel(16 * 4);
    let lifted = numbers
        .call(&mut store, (16,))
        .await
        .expect("sixteen numbers lift for their bytes");
    assert_eq!(lifted.len(), 16);

    let error = numbers
        .call(&mut store, (17,))
        .await
        .expect_err("seventeen numbers pass the fuel");
    assert_budget_spent(&error);
}

/// A guest whose `async`-lifted export returns a list of `n` bytes
/// through `task.return`, whose flat parameters carry the list's
/// pointer and length.
const RETURNS: &[u8] = component!(
    r#"
    (component
      (core module $libc (memory (export "memory") 256))
      (core instance $libc (instantiate $libc))
      (core func $task-return
        (canon task.return (result (list u8)) (memory (core memory $libc "memory"))))
      (core module $m
        (import "" "task.return" (func $task-return (param i32 i32)))
        (func (export "run") (param $n i32) (result i32)
          (call $task-return (i32.const 16) (local.get $n))
          (i32.const 0))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          unreachable))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "list") async (param "n" u32) (result (list u8))
        (canon lift (core func $i "run") async
          (memory (core memory $libc "memory"))
          (callback (core func $i "run-callback")))))
    "#
);

#[wcmp_macros::test]
async fn it_charges_the_list_a_task_returns() {
    let (mut store, instance) = instantiate(RETURNS).await;
    let n = BUDGET / ELEMENT;
    let results = call(&mut store, &instance, "list", &[n])
        .await
        .expect("a returned list of exactly the budget lifts");
    assert_eq!(length(&results[0]), n);

    let error = call(&mut store, &instance, "list", &[n + 1])
        .await
        .expect_err("a returned list one element past the budget is refused");
    assert_budget_spent_beneath(&error);
}

/// A guest that hands a host import two lists of `n` bytes as its
/// arguments, both over the same bytes of its memory. The four flat
/// parameters fit the import's flat slots, so the lists cross on the
/// flat path.
const SENDER: &[u8] = component!(
    r#"
    (component
      (type $iface (instance
        (export "take" (func (param "a" (list u8)) (param "b" (list u8))))))
      (import "wcmp-tests:host/bytes@0.1.0" (instance $imports (type $iface)))
      (alias export $imports "take" (func $take))
      (core module $libc
        (memory (export "memory") 33))
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
async fn it_charges_both_lists_a_guest_passes_a_host_import() {
    // Two lists of half the budget's elements each spend all of it,
    // and two of one element more pass it on the second list.
    let engine = Engine::new().expect("engine");
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
    let received = Arc::new(Mutex::new(Vec::new()));
    let seen = received.clone();
    linker
        .instance(&iface)
        .func_new("take", signature, move |_, args, _| {
            let lengths: Vec<usize> = args.iter().map(length).collect();
            seen.lock().expect("the record").push(lengths);
            Ok(())
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");

    let n = BUDGET / (2 * ELEMENT);
    call(&mut store, &instance, "send", &[n])
        .await
        .expect("two lists of half the budget each fit it");
    let error = call(&mut store, &instance, "send", &[n + 1])
        .await
        .expect_err("two lists of one element more pass it");
    assert_budget_spent_beneath(&error);
    assert_eq!(
        *received.lock().expect("the record"),
        [vec![n, n]],
        "only the lists inside the budget reached the host"
    );
}
