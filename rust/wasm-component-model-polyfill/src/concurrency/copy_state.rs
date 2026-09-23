//! How far one stream or future end is through its copies.

/// How far one stream or future end is through its copies. The
/// states are the reference's `CopyState`.
///
/// A copy is one read or one write on one end. An end starts `Idle`,
/// is `Copying` while a read or write it started has not completed,
/// and is `Cancelling` while a cancel of that copy has not completed.
/// It is `Done` once it can make no further copy: the other end was
/// dropped, or it is a future end that already read or wrote its one
/// value. A `Done` end accepts only a drop.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CopyState {
    /// No copy is in progress and another may start.
    Idle,
    /// A read or write on the end has started and not completed.
    ///
    /// Nothing moves an end here yet: the built-ins that start a copy
    /// do.
    #[allow(dead_code)]
    Copying,
    /// A cancel of the end's copy has started and not completed.
    ///
    /// Nothing moves an end here yet: the built-ins that cancel a copy
    /// do.
    #[allow(dead_code)]
    Cancelling,
    /// The end can make no further copy, and accepts only a drop.
    ///
    /// Nothing moves an end here yet: a completed future copy and a
    /// copy that finds the other end dropped do.
    #[allow(dead_code)]
    Done,
}

impl CopyState {
    /// Whether a copy on the end is in progress, being either copied
    /// or cancelled. Such an end cannot be dropped, and cannot cross a
    /// boundary.
    pub fn busy(self) -> bool {
        matches!(self, Self::Copying | Self::Cancelling)
    }
}
