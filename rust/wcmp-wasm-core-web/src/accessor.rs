//! The generated module through which the host reaches one guest memory.

use js_sys::{Function, Object, WebAssembly};
use wasm_bindgen::JsCast;
use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, FunctionSection, ImportSection,
    InstructionSink, TypeSection, ValType,
};
use wcmp_wasm_core::{MemoryType, Result};

use crate::code::{self, Side};
use crate::dispatcher::{self, Dispatcher};
use crate::errors;
use crate::js;

/// The functions of a generated accessor module over one guest memory.
///
/// Rust on `wasm32` addresses only memory 0 of the polyfill's own
/// instance, and a guest memory is another `WebAssembly.Memory`. So the
/// host cannot lend guest bytes to Rust, and it reaches a guest memory
/// through a small module that imports it:
///
/// ```text
/// (module
///   (import "" "memory" (memory $guest ...))
///   (import "" "host" (memory $host ...))       ;; where `multi_memory` is declared
///   (func (export "size") (result i64) ...)     ;; the size in bytes
///   (func (export "load8") (param i64) (result i32) ...)    ;; and load16, load32
///   (func (export "load64") (param i64) (result i64) ...)
///   (func (export "store8") (param i64 i32) ...)            ;; and store16, store32
///   (func (export "store64") (param i64 i64) ...)
///   (func (export "copy") (param i64 i64 i64) ...)          ;; within the guest memory
///   (func (export "read") (param i64 i64 i64) ...)          ;; guest to host, with $host
///   (func (export "write") (param i64 i64 i64) ...))        ;; host to guest, with $host
/// ```
///
/// Each function sits in the table of the [`Dispatcher`], so Rust calls
/// it through a function pointer, from WebAssembly to WebAssembly, with no
/// JavaScript frame. The scalar loads and stores are little-endian, and
/// reach a shared memory one byte at a time, each byte atomically.
///
/// Where the browser declares `multi_memory`, the module imports the
/// polyfill's own memory too, and `read` and `write` copy between the two
/// memories. Over an unshared memory, each is one `memory.copy`. Where the
/// browser lacks `multi_memory`, the module has neither, and the host
/// copies a range between a guest memory and its own with JavaScript.
///
/// No function checks its range: the host checks every range against
/// `size` before it calls, and a memory never shrinks.
pub struct Accessor {
    slots: Vec<u32>,
    shared: bool,
    bulk: bool,
}

/// The functions of an accessor, in the order of their slots.
#[derive(Clone, Copy)]
enum Op {
    Size,
    Load8,
    Load16,
    Load32,
    Load64,
    Store8,
    Store16,
    Store32,
    Store64,
    Copy,
    Read,
    Write,
}

impl Op {
    /// The operations of an accessor with `read` and `write` where `bulk`.
    fn all(bulk: bool) -> &'static [Op] {
        use Op::*;
        const ALL: [Op; 12] = [
            Size, Load8, Load16, Load32, Load64, Store8, Store16, Store32, Store64, Copy, Read,
            Write,
        ];
        if bulk { &ALL } else { &ALL[..10] }
    }

    /// The name of the export of the operation.
    fn name(self) -> &'static str {
        match self {
            Op::Size => "size",
            Op::Load8 => "load8",
            Op::Load16 => "load16",
            Op::Load32 => "load32",
            Op::Load64 => "load64",
            Op::Store8 => "store8",
            Op::Store16 => "store16",
            Op::Store32 => "store32",
            Op::Store64 => "store64",
            Op::Copy => "copy",
            Op::Read => "read",
            Op::Write => "write",
        }
    }

    /// The index of the operation's type in the type section.
    fn ty(self) -> u32 {
        match self {
            Op::Size => 0,
            Op::Load8 | Op::Load16 | Op::Load32 => 1,
            Op::Load64 => 2,
            Op::Store8 | Op::Store16 | Op::Store32 => 3,
            Op::Store64 => 4,
            Op::Copy | Op::Read | Op::Write => 5,
        }
    }
}

impl Accessor {
    /// The accessor of `memory`, whose type is `ty`, with `read` and
    /// `write` where `bulk`, which the browser's `multi_memory` permits.
    pub fn new(memory: &WebAssembly::Memory, ty: &MemoryType, bulk: bool) -> Result<Self> {
        let guest = Side {
            index: 0,
            is_64: ty.is_64(),
            shared: ty.is_shared(),
        };
        let host = bulk.then(|| Side {
            index: 1,
            is_64: false,
            shared: host_memory()
                .buffer()
                .is_instance_of::<js_sys::SharedArrayBuffer>(),
        });
        let bytes = generate(guest, ty.maximum(), host);
        let names = js::object(&[("memory", memory.clone().into())])
            .map_err(|error| errors::call(&error))?;
        if bulk {
            js::set(&names, "host", &host_memory()).map_err(|error| errors::call(&error))?;
        }
        let imports = js::object(&[("", names.into())]).map_err(|error| errors::call(&error))?;
        let exports = instantiate(&bytes, &imports)?;
        let mut accessor = Self {
            slots: Vec::new(),
            shared: guest.shared,
            bulk,
        };
        // Each slot taken is in `accessor.slots`, so the accessor releases
        // it on drop, even where a later one fails.
        for op in Op::all(bulk) {
            let function = export(&exports, op.name())?;
            accessor.slots.push(Dispatcher::register(&function)?);
        }
        Ok(accessor)
    }

    /// Whether the memory is shared.
    pub fn shared(&self) -> bool {
        self.shared
    }

    /// Whether the accessor has `read` and `write`.
    pub fn bulk(&self) -> bool {
        self.bulk
    }

    /// The size of the memory, in bytes.
    pub fn size(&self) -> u64 {
        dispatcher::size(self.slot(Op::Size))
    }

    /// The byte at `offset`.
    pub fn load8(&self, offset: u64) -> u8 {
        dispatcher::load(self.slot(Op::Load8), offset) as u8
    }

    /// The little-endian `u16` at `offset`.
    pub fn load16(&self, offset: u64) -> u16 {
        dispatcher::load(self.slot(Op::Load16), offset) as u16
    }

    /// The little-endian `u32` at `offset`.
    pub fn load32(&self, offset: u64) -> u32 {
        dispatcher::load(self.slot(Op::Load32), offset)
    }

    /// The little-endian `u64` at `offset`.
    pub fn load64(&self, offset: u64) -> u64 {
        dispatcher::load64(self.slot(Op::Load64), offset)
    }

    /// Stores the byte `value` at `offset`.
    pub fn store8(&self, offset: u64, value: u8) {
        dispatcher::store(self.slot(Op::Store8), offset, u32::from(value));
    }

    /// Stores `value` at `offset`, little-endian.
    pub fn store16(&self, offset: u64, value: u16) {
        dispatcher::store(self.slot(Op::Store16), offset, u32::from(value));
    }

    /// Stores `value` at `offset`, little-endian.
    pub fn store32(&self, offset: u64, value: u32) {
        dispatcher::store(self.slot(Op::Store32), offset, value);
    }

    /// Stores `value` at `offset`, little-endian.
    pub fn store64(&self, offset: u64, value: u64) {
        dispatcher::store64(self.slot(Op::Store64), offset, value);
    }

    /// Copies the `len` bytes at `source` to `destination`, both of this
    /// memory, as if through a buffer.
    pub fn copy(&self, destination: u64, source: u64, len: u64) {
        dispatcher::copy(self.slot(Op::Copy), destination, source, len);
    }

    /// Copies the bytes at `offset` into `buffer`, where the accessor has
    /// `read`.
    ///
    /// `buffer` lies in the polyfill's own memory, at the address of its
    /// first byte.
    pub fn read(&self, offset: u64, buffer: &mut [u8]) {
        let address = buffer.as_mut_ptr() as usize as u64;
        dispatcher::copy(self.slot(Op::Read), address, offset, buffer.len() as u64);
    }

    /// Copies `bytes` to `offset`, where the accessor has `write`.
    pub fn write(&self, offset: u64, bytes: &[u8]) {
        let address = bytes.as_ptr() as usize as u64;
        dispatcher::copy(self.slot(Op::Write), offset, address, bytes.len() as u64);
    }

    /// The slot of `op` in the table of the dispatcher.
    ///
    /// An accessor without `read` and `write` has no slot for them. The
    /// store asks for one only where [`Accessor::bulk`] holds, and an
    /// index past the table traps, so the index of a missing slot is the
    /// largest a table can have.
    fn slot(&self, op: Op) -> u32 {
        self.slots.get(op as usize).copied().unwrap_or(u32::MAX)
    }
}

impl Drop for Accessor {
    fn drop(&mut self) {
        Dispatcher::release(&self.slots);
    }
}

/// The polyfill's own memory.
fn host_memory() -> WebAssembly::Memory {
    wasm_bindgen::memory().unchecked_into()
}

/// The exports of the generated module `bytes`, instantiated with
/// `imports`.
///
/// A generated module is small, so it compiles and instantiates
/// synchronously.
pub fn instantiate(bytes: &[u8], imports: &Object) -> Result<Object> {
    let module = WebAssembly::Module::new(&js_sys::Uint8Array::from(bytes).into())
        .map_err(|error| errors::backend(errors::message(&error)))?;
    Ok(WebAssembly::Instance::new(&module, imports)
        .map_err(|error| errors::backend(errors::message(&error)))?
        .exports())
}

/// The function `exports` names `name`.
pub fn export(exports: &Object, name: &str) -> Result<Function> {
    js::get(exports, name)
        .ok()
        .and_then(|value| value.dyn_into::<Function>().ok())
        .ok_or_else(|| errors::backend(format!("the generated module exports no `{name}`")))
}

/// The type of each function of an accessor, by the index [`Op::ty`]
/// gives.
fn types() -> TypeSection {
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I64]);
    types.ty().function([ValType::I64], [ValType::I32]);
    types.ty().function([ValType::I64], [ValType::I64]);
    types.ty().function([ValType::I64, ValType::I32], []);
    types.ty().function([ValType::I64, ValType::I64], []);
    types
        .ty()
        .function([ValType::I64, ValType::I64, ValType::I64], []);
    types
}

/// The accessor module of the memory `guest`, whose type declares the
/// maximum `maximum`, with `read` and `write` over the memory `host` where
/// there is one.
fn generate(guest: Side, maximum: Option<u64>, host: Option<Side>) -> Vec<u8> {
    let mut module = wasm_encoder::Module::new();
    module.section(&types());
    let mut imports = ImportSection::new();
    imports.import("", "memory", EntityType::Memory(guest.import(maximum)));
    if let Some(host) = host {
        imports.import("", "host", EntityType::Memory(host.import(None)));
    }
    module.section(&imports);
    let ops = Op::all(host.is_some());
    let mut functions = FunctionSection::new();
    for op in ops {
        functions.function(op.ty());
    }
    module.section(&functions);
    let mut exports = ExportSection::new();
    for (index, op) in ops.iter().enumerate() {
        exports.export(op.name(), ExportKind::Func, index as u32);
    }
    module.section(&exports);
    let mut code = CodeSection::new();
    for op in ops {
        let function = match (op, host) {
            (Op::Copy, _) => code::copy(guest, guest),
            (Op::Read, Some(host)) => code::copy(host, guest),
            (Op::Write, Some(host)) => code::copy(guest, host),
            (op, _) => scalar(*op, guest),
        };
        code.function(&function);
    }
    module.section(&code);
    module.finish()
}

/// The function of the scalar operation `op`, or of `size`, on the memory
/// `guest`.
fn scalar(op: Op, guest: Side) -> wasm_encoder::Function {
    let mut function = wasm_encoder::Function::new([]);
    let mut code = function.instructions();
    match op {
        Op::Size => {
            code.memory_size(guest.index);
            if !guest.is_64 {
                code.i64_extend_i32_u();
            }
            code.i64_const(16).i64_shl();
        }
        Op::Load8 => load(&mut code, guest, 1),
        Op::Load16 => load(&mut code, guest, 2),
        Op::Load32 => load(&mut code, guest, 4),
        Op::Load64 => load(&mut code, guest, 8),
        Op::Store8 => store(&mut code, guest, 1),
        Op::Store16 => store(&mut code, guest, 2),
        Op::Store32 => store(&mut code, guest, 4),
        Op::Store64 => store(&mut code, guest, 8),
        // The copies have bodies of their own, from `code::copy`.
        Op::Copy | Op::Read | Op::Write => {
            code.unreachable();
        }
    }
    code.end();
    function
}

/// Loads the little-endian number of `width` bytes at the address in
/// parameter 0: an `i32` for up to 4 bytes, and an `i64` for 8.
///
/// Over a shared memory, it reads one byte at a time, each atomically, and
/// assembles the number.
fn load(code: &mut InstructionSink<'_>, guest: Side, width: u64) {
    let wide = width == 8;
    if !guest.shared {
        guest.address(code, 0);
        let memarg = guest.memarg(0);
        match width {
            1 => code.i32_load8_u(memarg),
            2 => code.i32_load16_u(memarg),
            4 => code.i32_load(memarg),
            _ => code.i64_load(memarg),
        };
        return;
    }
    for byte in 0..width {
        guest.address(code, 0);
        let memarg = guest.memarg(byte);
        if wide {
            code.i64_atomic_load8_u(memarg);
        } else {
            code.i32_atomic_load8_u(memarg);
        }
        if byte > 0 {
            if wide {
                code.i64_const(8 * byte as i64).i64_shl().i64_or();
            } else {
                code.i32_const(8 * byte as i32).i32_shl().i32_or();
            }
        }
    }
}

/// Stores the low `width` bytes of parameter 1, little-endian, at the
/// address in parameter 0.
///
/// Over a shared memory, it writes one byte at a time, each atomically.
fn store(code: &mut InstructionSink<'_>, guest: Side, width: u64) {
    let wide = width == 8;
    if !guest.shared {
        guest.address(code, 0);
        code.local_get(1);
        let memarg = guest.memarg(0);
        match width {
            1 => code.i32_store8(memarg),
            2 => code.i32_store16(memarg),
            4 => code.i32_store(memarg),
            _ => code.i64_store(memarg),
        };
        return;
    }
    for byte in 0..width {
        guest.address(code, 0);
        code.local_get(1);
        let memarg = guest.memarg(byte);
        if wide {
            code.i64_const(8 * byte as i64)
                .i64_shr_u()
                .i64_atomic_store8(memarg);
        } else {
            code.i32_const(8 * byte as i32)
                .i32_shr_u()
                .i32_atomic_store8(memarg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use js_sys::{Array, Reflect};
    use wasm_bindgen::JsValue;
    use wasm_bindgen::closure::Closure;
    use wasmparser::{Operator, Parser, Payload};

    /// A new unshared memory of `pages` pages, and its type.
    fn memory(pages: u32) -> (WebAssembly::Memory, MemoryType) {
        let descriptor = js::object(&[("initial", JsValue::from_f64(f64::from(pages)))])
            .expect("the descriptor is an object");
        let memory = WebAssembly::Memory::new(&descriptor).expect("the browser makes a memory");
        (memory, MemoryType::new(pages, None))
    }

    /// The operators of each function body of the module `bytes`, by the
    /// name of its export.
    fn bodies(bytes: &[u8]) -> Vec<(String, Vec<String>)> {
        let mut names = Vec::new();
        let mut bodies = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            match payload.expect("the generated module parses") {
                Payload::ExportSection(exports) => {
                    for export in exports {
                        names.push(export.expect("the export parses").name.to_string());
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let operators = body
                        .get_operators_reader()
                        .expect("the body parses")
                        .into_iter()
                        .map(|operator| {
                            let operator = operator.expect("the operator parses");
                            let name = format!("{operator:?}");
                            match operator {
                                Operator::MemoryCopy { .. } => "memory.copy".to_string(),
                                _ => name,
                            }
                        })
                        .collect();
                    bodies.push(operators);
                }
                _ => {}
            }
        }
        names.into_iter().zip(bodies).collect()
    }

    /// Whether `operators` is one `memory.copy` and the steps that feed
    /// it, with no loop and no call.
    fn one_memory_copy(operators: &[String]) -> bool {
        operators
            .iter()
            .filter(|name| *name == "memory.copy")
            .count()
            == 1
            && !operators.iter().any(|name| {
                name.starts_with("Loop") || name.starts_with("Call") || name.contains("Atomic")
            })
    }

    #[wcmp_macros::test]
    fn it_reads_and_writes_through_the_table_of_the_dispatcher() {
        let (memory, ty) = memory(1);
        let accessor = Accessor::new(&memory, &ty, false).expect("the accessor loads");

        assert_eq!(accessor.size(), 65_536);
        accessor.store32(17, 0x1122_3344);
        assert_eq!(accessor.load32(17), 0x1122_3344, "an unaligned load");
        assert_eq!(accessor.load16(17), 0x3344);
        assert_eq!(accessor.load8(18), 0x33);
        accessor.store64(65_528, u64::MAX - 1);
        assert_eq!(accessor.load64(65_528), u64::MAX - 1);
        let view = js_sys::Uint8Array::new(&memory.buffer());
        assert_eq!(view.get_index(17), 0x44, "the store is little-endian");
        accessor.copy(18, 17, 4);
        assert_eq!(accessor.load32(18), 0x1122_3344);
    }

    /// A load past the end of the memory, which traps inside the accessor.
    /// The host never makes one: this test makes one to read the stack at
    /// the moment of the load.
    #[inline(never)]
    fn load_past_the_end(accessor: &Accessor) -> u32 {
        accessor.load32(1 << 20)
    }

    #[wcmp_macros::test]
    fn it_loads_a_scalar_with_no_javascript_frame_on_the_stack() {
        let (memory, ty) = memory(1);
        let accessor = std::rc::Rc::new(Accessor::new(&memory, &ty, false).expect("it loads"));
        let error_class = js::get(&js_sys::global(), "Error").expect("the page has `Error`");
        js::set(&error_class, "stackTraceLimit", &JsValue::from_f64(100.0))
            .expect("the limit is writable");

        // The trap unwinds the Rust frames of the closure, and stops at
        // `Reflect.apply`, which hands it back as an error.
        let load = Closure::<dyn Fn()>::new({
            let accessor = accessor.clone();
            move || {
                load_past_the_end(&accessor);
            }
        });
        let error = Reflect::apply(
            load.as_ref().unchecked_ref::<Function>(),
            &JsValue::UNDEFINED,
            &Array::new(),
        )
        .expect_err("the load past the end traps");
        assert!(
            error.is_instance_of::<WebAssembly::RuntimeError>(),
            "{error:?}"
        );
        let stack = js::get(&error, "stack")
            .ok()
            .and_then(|stack| stack.as_string())
            .expect("the trap has a stack");
        let frames = stack
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("at "))
            .collect::<Vec<_>>();
        let caller = frames
            .iter()
            .position(|frame| frame.contains("load_past_the_end"))
            .unwrap_or_else(|| panic!("the Rust caller is on the stack:\n{stack}"));
        // A frame of WebAssembly names its module and its function, as
        // `wasm://wasm/1a2b3c4d:wasm-function[3]:0x9a` for a module made
        // from bytes, or with the URL of the polyfill's own module. A frame
        // of JavaScript names a script, a line, and a column.
        let module_of = |frame: &str| {
            let end = frame.find(":wasm-function[")?;
            let start = frame[..end].rfind(['(', ' ']).map_or(0, |index| index + 1);
            Some(frame[start..end].to_string())
        };
        let modules = frames[..=caller]
            .iter()
            .map(|frame| {
                module_of(frame).unwrap_or_else(|| {
                    panic!("a JavaScript frame lies between the Rust caller and the load: {frame}\n{stack}")
                })
            })
            .collect::<Vec<_>>();
        let polyfill = &modules[caller];
        let accessor = &modules[0];
        assert_ne!(
            accessor, polyfill,
            "the load runs in the accessor:\n{stack}"
        );
        assert!(
            modules
                .iter()
                .any(|module| module != accessor && module != polyfill),
            "the trampoline of the dispatcher lies between the two:\n{stack}"
        );
    }

    #[wcmp_macros::test]
    fn it_copies_in_bulk_with_one_memory_copy_over_an_unshared_memory() {
        let unshared = Side {
            index: 0,
            is_64: false,
            shared: false,
        };
        let host = Side {
            index: 1,
            is_64: false,
            shared: false,
        };
        for (name, body) in bodies(&generate(unshared, None, Some(host))) {
            if ["copy", "read", "write"].contains(&name.as_str()) {
                assert!(one_memory_copy(&body), "`{name}`: {body:?}");
            } else {
                assert!(!body.iter().any(|op| op == "memory.copy"), "`{name}`");
            }
        }
        let names = bodies(&generate(unshared, None, None))
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert!(
            !names.iter().any(|name| name == "read" || name == "write"),
            "without `multi_memory`, the accessor has no bulk copy to the host: {names:?}"
        );

        // A shared memory is copied one byte at a time, atomically.
        let shared = Side {
            shared: true,
            ..unshared
        };
        for (name, body) in bodies(&generate(shared, Some(1), Some(host))) {
            if ["copy", "read", "write"].contains(&name.as_str()) {
                assert!(
                    !body.iter().any(|op| op == "memory.copy"),
                    "`{name}`: {body:?}"
                );
                assert!(
                    body.iter().any(|op| op.starts_with("I32AtomicLoad8U"))
                        || body.iter().any(|op| op.starts_with("I32AtomicStore8")),
                    "`{name}`: {body:?}"
                );
            }
        }
    }

    #[wcmp_macros::test]
    fn it_releases_its_slots_for_the_next_accessor() {
        let (memory, ty) = memory(1);
        let first = Accessor::new(&memory, &ty, false).expect("the accessor loads");
        let mut slots = first.slots.clone();
        drop(first);
        let second = Accessor::new(&memory, &ty, false).expect("the accessor loads");
        let mut again = second.slots.clone();
        slots.sort_unstable();
        again.sort_unstable();
        assert_eq!(slots, again, "the second accessor takes the released slots");
        assert_eq!(second.size(), 65_536);
    }
}
