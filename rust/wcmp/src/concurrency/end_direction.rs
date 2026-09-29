//! Which way the values move through one stream or future end.

/// Which way the values move through one stream or future end: out
/// of a readable end, into a writable one.
///
/// The two ends of one stream or future are one of each. A guest
/// that creates the pair holds both, and the readable end is the one
/// that crosses a boundary as a value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EndDirection {
    /// The end a reader holds: `stream.read` and `future.read` copy
    /// values out of it.
    Readable,
    /// The end a writer holds: `stream.write` and `future.write` copy
    /// values into it.
    Writable,
}
