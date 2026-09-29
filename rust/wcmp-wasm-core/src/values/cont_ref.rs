//! A continuation reference.

handle! {
    /// A `contref`: a reference to a continuation of stack switching.
    ///
    /// The reference is opaque. The host can hold it, test it for null (a
    /// null is `None` in a [`Val`](crate::Val)), and give it back to a guest
    /// of the same store.
    ContRef
}
