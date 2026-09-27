//! One error context: a debug message the guests that hold it share.

/// The record of one error context.
///
/// The store keeps one record per error context, whichever instances
/// hold a handle to it. The record holds the debug message exactly
/// as `error-context.new` read it, and the count of the guest
/// handles that name it. `error-context.new` creates the record with
/// a count of one, and `error-context.drop` subtracts one; the
/// record leaves the store when the count reaches zero.
pub struct ErrorContextRecord {
    /// The debug message, as the guest wrote it.
    pub debug_message: String,
    /// How many guest handles name the record.
    pub handle_count: u32,
}

impl ErrorContextRecord {
    /// Construct the record `error-context.new` creates: one handle
    /// names it.
    pub fn new(debug_message: String) -> Self {
        Self {
            debug_message,
            handle_count: 1,
        }
    }
}
