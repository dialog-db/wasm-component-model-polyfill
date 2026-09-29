//! The generated module that copies from one guest memory to another.

use js_sys::WebAssembly;
use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, FunctionSection, ImportSection,
    TypeSection, ValType,
};
use wcmp_wasm_core::{MemoryType, Result};

use crate::accessor;
use crate::code::{self, Side};
use crate::dispatcher::{self, Dispatcher};
use crate::errors;
use crate::js;

/// A generated module that imports two guest memories, where the browser
/// declares `multi_memory`, and copies from the first to the second:
///
/// ```text
/// (module
///   (import "" "source" (memory $source ...))
///   (import "" "destination" (memory $destination ...))
///   (func (export "copy") (param i64 i64 i64) ...))
/// ```
///
/// Between two unshared memories, a copy is one `memory.copy`. Its function
/// sits in the table of the [`Dispatcher`], so Rust calls it with no
/// JavaScript frame. The two memories can be one: `memory.copy` copies
/// between overlapping ranges as if through a buffer.
pub struct Bridge {
    slot: u32,
    narrow: bool,
}

impl Bridge {
    /// The bridge from `source`, of type `from`, to `destination`, of type
    /// `to`.
    pub fn new(
        (source, from): (&WebAssembly::Memory, &MemoryType),
        (destination, to): (&WebAssembly::Memory, &MemoryType),
    ) -> Result<Self> {
        let from_side = Side {
            index: 0,
            is_64: from.is_64(),
            shared: from.is_shared(),
        };
        let to_side = Side {
            index: 1,
            is_64: to.is_64(),
            shared: to.is_shared(),
        };
        let bytes = generate((from_side, from.maximum()), (to_side, to.maximum()));
        let names = js::object(&[
            ("source", source.clone().into()),
            ("destination", destination.clone().into()),
        ])
        .map_err(|error| errors::call(&error))?;
        let imports = js::object(&[("", names.into())]).map_err(|error| errors::call(&error))?;
        let exports = accessor::instantiate(&bytes, &imports)?;
        let slot = Dispatcher::register(&accessor::export(&exports, "copy")?)?;
        Ok(Self {
            slot,
            narrow: !(from.is_64() && to.is_64()),
        })
    }

    /// Copies the `len` bytes at `source` of the first memory to
    /// `destination` of the second.
    ///
    /// The length of a `memory.copy` has the smaller address type of its
    /// two memories. A range of 4 GiB, the whole of a full memory addressed
    /// with 32-bit numbers, does not fit one, so it takes two halves.
    pub fn copy(&self, destination: u64, source: u64, len: u64) {
        if self.narrow && len > u64::from(u32::MAX) {
            let half = len / 2;
            dispatcher::copy(self.slot, destination, source, half);
            dispatcher::copy(self.slot, destination + half, source + half, len - half);
            return;
        }
        dispatcher::copy(self.slot, destination, source, len);
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        Dispatcher::release(&[self.slot]);
    }
}

/// The bridge module from the memory `from`, whose type declares the
/// maximum it carries, to the memory `to`.
fn generate(from: (Side, Option<u64>), to: (Side, Option<u64>)) -> Vec<u8> {
    let mut module = wasm_encoder::Module::new();
    let mut types = TypeSection::new();
    types
        .ty()
        .function([ValType::I64, ValType::I64, ValType::I64], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("", "source", EntityType::Memory(from.0.import(from.1)));
    imports.import("", "destination", EntityType::Memory(to.0.import(to.1)));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("copy", ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    code.function(&code::copy(to.0, from.0));
    module.section(&code);
    module.finish()
}
