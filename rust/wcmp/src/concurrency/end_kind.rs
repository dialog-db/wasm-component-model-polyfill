//! Which of the four kinds of stream or future end one end is.

/// Which of the four kinds of stream or future end one end is: the
/// readable or the writable end of a stream, or of a future.
///
/// A handle-table entry for an end is of one of these kinds, and each
/// built-in that takes an end names the kind it works on. A trap an
/// end raises names the kind too, because Wasmtime's messages differ
/// per kind and the conformance corpora match them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EndKind {
    /// The readable end of a stream.
    StreamReadable,
    /// The writable end of a stream.
    StreamWritable,
    /// The readable end of a future.
    FutureReadable,
    /// The writable end of a future.
    FutureWritable,
}

impl core::fmt::Display for EndKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::StreamReadable => write!(f, "readable end of a stream"),
            Self::StreamWritable => write!(f, "writable end of a stream"),
            Self::FutureReadable => write!(f, "readable end of a future"),
            Self::FutureWritable => write!(f, "writable end of a future"),
        }
    }
}
