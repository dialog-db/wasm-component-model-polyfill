//! A memory of a store, shared or not.

/// A memory of a store.
///
/// Wasmtime gives a shared memory a type of its own, which it lends only as
/// cells that must be reached through atomic operations. The runtime layer
/// has one memory type, so the store keeps either kind under one handle.
#[derive(Clone, Debug)]
pub enum MemoryObject {
    /// An unshared memory, which the store owns.
    Unshared(wasmtime::Memory),
    /// A shared memory, which another agent can write at any time.
    Shared(wasmtime::SharedMemory),
}

impl MemoryObject {
    /// The memory as an extern Wasmtime links.
    pub fn to_extern(&self) -> wasmtime::Extern {
        match self {
            MemoryObject::Unshared(memory) => wasmtime::Extern::Memory(*memory),
            MemoryObject::Shared(memory) => wasmtime::Extern::SharedMemory(memory.clone()),
        }
    }
}
