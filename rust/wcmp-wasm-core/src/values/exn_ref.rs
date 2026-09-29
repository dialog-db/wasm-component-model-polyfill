//! An exception reference.

handle! {
    /// An `exnref`: a reference to an exception.
    ///
    /// The reference is opaque. The host can hold it, test it for null (a
    /// null is `None` in a [`Val`](crate::Val)), and give it back to a guest
    /// of the same store. It cannot read the payload.
    ExnRef
}
