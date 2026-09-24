//! The buffer of one copy in progress on a guest end.

use std::sync::{Arc, Mutex};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::executor::ir::CanonOptions;
use crate::types::ValueType;

use super::instance_id::InstanceId;

/// The buffer of one copy in progress on a guest end: where in the
/// guest's memory the values are read from or written to, how many
/// the copy asked for, and how many it has moved so far. The
/// reference names it `BufferGuestImpl`.
///
/// An end holds one from the moment a read or a write on it starts
/// until the event that reports the copy is delivered, so the event
/// reads the progress the copy made by then. The buffer also carries
/// what a boundary context over the guest's memory is built from —
/// the built-in's canon options and the runtime state of the
/// instantiation they index — because the copy that pairs with this
/// one can run in a later call of another built-in, and that call
/// builds this side's context as well as its own.
pub struct CopyBuffer {
    /// The type of each value the copy moves, as the built-in that
    /// started it declared it, or `None` for a stream that carries
    /// no values.
    pub payload: Option<ValueType>,
    /// The canon options of the built-in that started the copy: the
    /// memory the buffer lives in, and the `realloc` a value lowered
    /// into it may call.
    pub options: Arc<CanonOptions>,
    /// The runtime state of the instantiation the options index.
    pub abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The component instance whose built-in started the copy, the
    /// reference's `pending_inst` while the copy is the pending side.
    pub instance: InstanceId,
    /// Whether the payload is a number type or absent, the
    /// reference's `none_or_number_type`: the one case in which a
    /// read and a write from one instance may meet.
    pub number_or_none: bool,
    /// The guest's pointer to the first value of the copy.
    pub pointer: u32,
    /// The count of values the copy asked for.
    pub length: u32,
    /// The count of values the copy has moved so far.
    pub progress: u32,
}

impl CopyBuffer {
    /// The count of values the copy can still move.
    pub fn remain(&self) -> u32 {
        self.length - self.progress
    }

    /// Whether the copy asked for no values at all, which is a
    /// readiness probe rather than a transfer.
    pub fn is_zero_length(&self) -> bool {
        self.length == 0
    }
}
