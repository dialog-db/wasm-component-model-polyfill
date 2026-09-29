//! The entries through which Rust calls a generated module with no
//! JavaScript frame.

use core::cell::RefCell;
use core::hint::black_box;

use js_sys::{Function, Object, Uint8Array, WebAssembly};
use wasm_bindgen::{JsCast, JsValue};
use wcmp_macros::wasm;
use wcmp_wasm_core::Result;

use crate::errors;
use crate::js;

/// The table of the functions of the generated modules, and the entries of
/// the polyfill's own function table that reach it.
///
/// Rust on `wasm32` calls a function pointer with `call_indirect` on the
/// function table of its own instance. A function of another instance can
/// sit in that table, and a call of it is then a call from WebAssembly to
/// WebAssembly, with no JavaScript frame. But the linker makes the table
/// of the polyfill as large as the functions of the polyfill, and no
/// larger: it cannot grow.
///
/// So the polyfill keeps one entry function for each signature an
/// accessor has, whose table slot the backend overwrites once. The new
/// occupant of each slot is a trampoline of a generated dispatch module,
/// which takes the index of a function in a table of its own and calls it
/// with `call_indirect`. That table grows. The backend puts each function
/// of each generated accessor in it, and names the function by its index.
///
/// ```text
/// Rust ──call_indirect──▶ trampoline ──call_indirect──▶ accessor function
///       (the polyfill's      (dispatch     (the dispatch   (a generated
///        own table)           module)       module's table)  module)
/// ```
///
/// The dispatcher belongs to the thread, as the polyfill's own table does.
/// A slot of a function stays taken until the owner of the function
/// releases it, and a released slot is taken again before the table grows.
pub struct Dispatcher {
    slots: WebAssembly::Table,
    free: Vec<u32>,
}

thread_local! {
    static DISPATCHER: RefCell<Option<Dispatcher>> = const { RefCell::new(None) };
}

/// The dispatch module: one table that grows, and one trampoline for each
/// signature of an accessor function, with the index of the function
/// first.
const DISPATCH: &[u8] = wasm!(
    r#"
    (module
      (type $size (func (result i64)))
      (type $load (func (param i64) (result i32)))
      (type $load64 (func (param i64) (result i64)))
      (type $store (func (param i64 i32)))
      (type $store64 (func (param i64 i64)))
      (type $copy (func (param i64 i64 i64)))
      (table $slots (export "slots") 0 funcref)
      (func (export "size") (param $slot i32) (result i64)
        local.get $slot
        call_indirect $slots (type $size))
      (func (export "load") (param $slot i32) (param i64) (result i32)
        local.get 1
        local.get $slot
        call_indirect $slots (type $load))
      (func (export "load64") (param $slot i32) (param i64) (result i64)
        local.get 1
        local.get $slot
        call_indirect $slots (type $load64))
      (func (export "store") (param $slot i32) (param i64 i32)
        local.get 1
        local.get 2
        local.get $slot
        call_indirect $slots (type $store))
      (func (export "store64") (param $slot i32) (param i64 i64)
        local.get 1
        local.get 2
        local.get $slot
        call_indirect $slots (type $store64))
      (func (export "copy") (param $slot i32) (param i64 i64 i64)
        local.get 1
        local.get 2
        local.get 3
        local.get $slot
        call_indirect $slots (type $copy)))
    "#
);

impl Dispatcher {
    /// Puts `function` in the table of the dispatcher, and returns its
    /// index there.
    ///
    /// The first call on a thread makes the dispatcher, and puts its
    /// trampolines in the polyfill's own table.
    pub fn register(function: &Function) -> Result<u32> {
        DISPATCHER.with(|dispatcher| {
            let mut dispatcher = dispatcher.borrow_mut();
            if dispatcher.is_none() {
                *dispatcher = Some(Self::install()?);
            }
            let dispatcher = dispatcher
                .as_mut()
                .ok_or_else(|| errors::backend("the dispatcher did not load"))?;
            let slot = match dispatcher.free.pop() {
                Some(slot) => slot,
                None => dispatcher
                    .slots
                    .grow(1)
                    .map_err(|error| errors::backend(errors::message(&error)))?,
            };
            dispatcher
                .slots
                .set(slot, function)
                .map_err(|error| errors::backend(errors::message(&error)))?;
            Ok(slot)
        })
    }

    /// Empties the slots `slots`, so the table no longer holds their
    /// functions, and the instance of each can be collected.
    pub fn release(slots: &[u32]) {
        // A thread that ends drops its dispatcher first, and its table
        // with it.
        let _ = DISPATCHER.try_with(|dispatcher| {
            let Ok(mut dispatcher) = dispatcher.try_borrow_mut() else {
                return;
            };
            let Some(dispatcher) = dispatcher.as_mut() else {
                return;
            };
            for slot in slots {
                let emptied = js::call_method(
                    &dispatcher.slots,
                    "set",
                    &[JsValue::from_f64(f64::from(*slot)), JsValue::NULL],
                );
                if emptied.is_ok() {
                    dispatcher.free.push(*slot);
                }
            }
        });
    }

    /// Makes the dispatcher, and puts each trampoline in the slot of its
    /// entry function in the polyfill's own table.
    fn install() -> Result<Self> {
        let module = WebAssembly::Module::new(&Uint8Array::from(DISPATCH).into())
            .map_err(|error| errors::backend(errors::message(&error)))?;
        let exports = WebAssembly::Instance::new(&module, &Object::new())
            .map_err(|error| errors::backend(errors::message(&error)))?
            .exports();
        let export = |name: &str| {
            js::get(&exports, name)
                .ok()
                .and_then(|value| value.dyn_into::<Function>().ok())
                .ok_or_else(|| errors::backend(format!("the dispatcher exports no `{name}`")))
        };
        let table = wasm_bindgen::function_table().unchecked_into::<WebAssembly::Table>();
        let entries: [(&str, usize); 6] = [
            ("size", size_entry as *const () as usize),
            ("load", load_entry as *const () as usize),
            ("load64", load64_entry as *const () as usize),
            ("store", store_entry as *const () as usize),
            ("store64", store64_entry as *const () as usize),
            ("copy", copy_entry as *const () as usize),
        ];
        for (name, slot) in entries {
            table
                .set(slot as u32, &export(name)?)
                .map_err(|error| errors::backend(errors::message(&error)))?;
        }
        let slots = js::get(&exports, "slots")
            .ok()
            .and_then(|value| value.dyn_into::<WebAssembly::Table>().ok())
            .ok_or_else(|| errors::backend("the dispatcher exports no table"))?;
        Ok(Self {
            slots,
            free: Vec::new(),
        })
    }
}

// The entry functions. Each one holds a slot of the polyfill's own table,
// which the dispatcher overwrites with the trampoline of its signature
// before any function of an accessor has an index. So none of these bodies
// runs: a call through the slot runs the trampoline. Each body is its own,
// so the compiler cannot fold two entries into one slot.

extern "C" fn size_entry(_slot: u32) -> u64 {
    not_installed(0)
}

extern "C" fn load_entry(_slot: u32, _offset: u64) -> u32 {
    not_installed(1)
}

extern "C" fn load64_entry(_slot: u32, _offset: u64) -> u64 {
    not_installed(2)
}

extern "C" fn store_entry(_slot: u32, _offset: u64, _value: u32) {
    not_installed(3)
}

extern "C" fn store64_entry(_slot: u32, _offset: u64, _value: u64) {
    not_installed(4)
}

extern "C" fn copy_entry(_slot: u32, _destination: u64, _source: u64, _len: u64) {
    not_installed(5)
}

/// The body of the entry function `entry`, which never runs once the
/// dispatcher is in place.
#[inline(never)]
#[cold]
fn not_installed(entry: u32) -> ! {
    black_box(entry);
    core::arch::wasm32::unreachable()
}

// The calls through the entries. `black_box` hides which function the
// pointer names, so the compiler emits `call_indirect` through the slot,
// and never a direct call of the entry's own body.

/// Calls the function at `slot` of type `[] -> [i64]`.
pub fn size(slot: u32) -> u64 {
    black_box(size_entry as extern "C" fn(u32) -> u64)(slot)
}

/// Calls the function at `slot` of type `[i64] -> [i32]`.
pub fn load(slot: u32, offset: u64) -> u32 {
    black_box(load_entry as extern "C" fn(u32, u64) -> u32)(slot, offset)
}

/// Calls the function at `slot` of type `[i64] -> [i64]`.
pub fn load64(slot: u32, offset: u64) -> u64 {
    black_box(load64_entry as extern "C" fn(u32, u64) -> u64)(slot, offset)
}

/// Calls the function at `slot` of type `[i64 i32] -> []`.
pub fn store(slot: u32, offset: u64, value: u32) {
    black_box(store_entry as extern "C" fn(u32, u64, u32))(slot, offset, value);
}

/// Calls the function at `slot` of type `[i64 i64] -> []`.
pub fn store64(slot: u32, offset: u64, value: u64) {
    black_box(store64_entry as extern "C" fn(u32, u64, u64))(slot, offset, value);
}

/// Calls the function at `slot` of type `[i64 i64 i64] -> []`.
pub fn copy(slot: u32, destination: u64, source: u64, len: u64) {
    black_box(copy_entry as extern "C" fn(u32, u64, u64, u64))(slot, destination, source, len);
}
