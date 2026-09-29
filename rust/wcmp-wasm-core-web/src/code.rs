//! The code that the generated modules run on a memory.

use wasm_encoder::{BlockType, Function, InstructionSink, MemArg};

/// One memory of a generated module, as the code of the module reaches it.
///
/// Every function of a generated module takes an offset as an `i64`, and
/// narrows it for a memory addressed with 32-bit numbers. The host checks
/// every range against the size of the memory before it calls, so an
/// offset always fits, and no function of a generated module traps.
///
/// Another agent can write a shared memory at any time, so the code reaches
/// each byte of a shared memory with an atomic access, as Wasmtime does for
/// its own shared memory.
#[derive(Clone, Copy, Debug)]
pub struct Side {
    /// The index of the memory in the generated module.
    pub index: u32,
    /// Whether the memory is addressed with 64-bit numbers.
    pub is_64: bool,
    /// Whether the memory is shared.
    pub shared: bool,
}

impl Side {
    /// The type under which a generated module imports this memory, whose
    /// maximum is `maximum` pages.
    ///
    /// The import asks for no minimum, and for a maximum only where the
    /// memory is shared, since a shared memory must declare one. The
    /// JavaScript API does not tell the maximum of a memory, so `maximum`
    /// is the one its type declares, which is at least the memory's own.
    pub fn import(&self, maximum: Option<u64>) -> wasm_encoder::MemoryType {
        let largest = if self.is_64 { 1 << 48 } else { 1 << 16 };
        wasm_encoder::MemoryType {
            minimum: 0,
            maximum: self.shared.then(|| maximum.unwrap_or(largest)),
            memory64: self.is_64,
            shared: self.shared,
            page_size_log2: None,
        }
    }

    /// The address in the `i64` local `local`, in the address type of the
    /// memory.
    pub fn address(&self, code: &mut InstructionSink<'_>, local: u32) {
        code.local_get(local);
        if !self.is_64 {
            code.i32_wrap_i64();
        }
    }

    /// The immediate of an access of this memory, `offset` bytes past its
    /// address. The access claims no alignment, because an offset of the
    /// host can have any.
    pub fn memarg(&self, offset: u64) -> MemArg {
        MemArg {
            offset,
            align: 0,
            memory_index: self.index,
        }
    }

    /// Loads the byte at the address on the stack, as an `i32`.
    fn load_byte(&self, code: &mut InstructionSink<'_>) {
        if self.shared {
            code.i32_atomic_load8_u(self.memarg(0));
        } else {
            code.i32_load8_u(self.memarg(0));
        }
    }

    /// Stores the byte of the `i32` on the stack at the address below it.
    fn store_byte(&self, code: &mut InstructionSink<'_>) {
        if self.shared {
            code.i32_atomic_store8(self.memarg(0));
        } else {
            code.i32_store8(self.memarg(0));
        }
    }

    /// The address `base + counter`, of the `i64` locals `base` and
    /// `counter`, in the address type of the memory.
    fn address_at(&self, code: &mut InstructionSink<'_>, base: u32, counter: u32) {
        code.local_get(base).local_get(counter).i64_add();
        if !self.is_64 {
            code.i32_wrap_i64();
        }
    }
}

/// The function of type `[i64 i64 i64] -> []` that copies the `len` bytes
/// at `source` of the memory `from` to `destination` of the memory `to`,
/// taking its parameters in that order: destination, source, len.
///
/// Where neither memory is shared, the body is one `memory.copy`. Its
/// length has the smaller address type of the two, so the host never asks
/// for 4 GiB or more where either memory is addressed with 32-bit numbers.
///
/// Where either memory is shared, the body copies one byte at a time,
/// with an atomic access on each shared side. It copies forward where the
/// destination lies at or before the source, and backward otherwise, as
/// `memory.copy` does, so a copy between overlapping ranges of one memory
/// reads each byte before it overwrites it.
pub fn copy(to: Side, from: Side) -> Function {
    const DESTINATION: u32 = 0;
    const SOURCE: u32 = 1;
    const LEN: u32 = 2;
    const COUNTER: u32 = 3;
    let mut function = Function::new([(1, wasm_encoder::ValType::I64)]);
    let mut code = function.instructions();
    if !to.shared && !from.shared {
        to.address(&mut code, DESTINATION);
        from.address(&mut code, SOURCE);
        code.local_get(LEN);
        if !(to.is_64 && from.is_64) {
            code.i32_wrap_i64();
        }
        code.memory_copy(to.index, from.index);
        code.end();
        return function;
    }
    let byte = |code: &mut InstructionSink<'_>| {
        to.address_at(code, DESTINATION, COUNTER);
        from.address_at(code, SOURCE, COUNTER);
        from.load_byte(code);
        to.store_byte(code);
    };
    code.local_get(DESTINATION)
        .local_get(SOURCE)
        .i64_le_u()
        .if_(BlockType::Empty);
    // Forward: the counter runs from 0 up to `len`.
    code.block(BlockType::Empty).loop_(BlockType::Empty);
    code.local_get(COUNTER).local_get(LEN).i64_eq().br_if(1);
    byte(&mut code);
    code.local_get(COUNTER)
        .i64_const(1)
        .i64_add()
        .local_set(COUNTER)
        .br(0)
        .end()
        .end();
    code.else_();
    // Backward: the counter runs from `len` down to 0.
    code.local_get(LEN).local_set(COUNTER);
    code.block(BlockType::Empty).loop_(BlockType::Empty);
    code.local_get(COUNTER).i64_eqz().br_if(1);
    code.local_get(COUNTER)
        .i64_const(1)
        .i64_sub()
        .local_set(COUNTER);
    byte(&mut code);
    code.br(0).end().end();
    code.end();
    code.end();
    function
}
