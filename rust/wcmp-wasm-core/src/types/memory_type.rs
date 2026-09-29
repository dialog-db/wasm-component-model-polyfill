//! The type of a memory.

/// The type of a memory: its limits in pages of 64 KiB, whether it is
/// addressed with 64-bit numbers, and whether it is shared.
///
/// A memory addressed with 64-bit numbers needs the
/// [`memory64`](crate::Capability::Memory64) capability, and a shared memory
/// needs [`threads`](crate::Capability::Threads).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemoryType {
    minimum: u64,
    maximum: Option<u64>,
    is_64: bool,
    shared: bool,
}

impl MemoryType {
    /// The size of a page, in bytes.
    pub const PAGE_SIZE: u64 = 65_536;

    /// An unshared memory addressed with 32-bit numbers.
    pub const fn new(minimum: u32, maximum: Option<u32>) -> Self {
        Self {
            minimum: minimum as u64,
            maximum: match maximum {
                Some(maximum) => Some(maximum as u64),
                None => None,
            },
            is_64: false,
            shared: false,
        }
    }

    /// An unshared memory addressed with 64-bit numbers.
    pub const fn new64(minimum: u64, maximum: Option<u64>) -> Self {
        Self {
            minimum,
            maximum,
            is_64: true,
            shared: false,
        }
    }

    /// A shared memory addressed with 32-bit numbers. A shared memory always
    /// has a maximum.
    pub const fn shared(minimum: u32, maximum: u32) -> Self {
        Self {
            minimum: minimum as u64,
            maximum: Some(maximum as u64),
            is_64: false,
            shared: true,
        }
    }

    /// A shared memory addressed with 64-bit numbers. A shared memory always
    /// has a maximum.
    pub const fn shared64(minimum: u64, maximum: u64) -> Self {
        Self {
            minimum,
            maximum: Some(maximum),
            is_64: true,
            shared: true,
        }
    }

    /// The least size of the memory, in pages.
    pub const fn minimum(&self) -> u64 {
        self.minimum
    }

    /// The greatest size of the memory, in pages, where it has one.
    pub const fn maximum(&self) -> Option<u64> {
        self.maximum
    }

    /// Whether the memory is addressed with 64-bit numbers.
    pub const fn is_64(&self) -> bool {
        self.is_64
    }

    /// Whether the memory is shared.
    pub const fn is_shared(&self) -> bool {
        self.shared
    }
}
