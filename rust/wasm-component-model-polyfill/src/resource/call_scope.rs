//! The per-call bookkeeping the canonical ABI keeps for borrows.

use super::table_id::TableId;

/// One entry of the store's call stack. A scope is pushed when a call
/// crosses the host boundary in either direction and popped when it
/// returns; the state mirrors Wasmtime's per-call context.
#[derive(Debug, Default)]
pub struct CallScope {
    /// The borrows lowered into the guest during this call that the
    /// guest has not dropped yet. Must be zero when the call ends.
    pub borrow_count: u32,
    /// The owning entries whose borrows were lifted into the host
    /// during this call, as `(table, index)`. Each lend is undone
    /// when the call ends.
    pub lenders: Vec<(TableId, u32)>,
}
