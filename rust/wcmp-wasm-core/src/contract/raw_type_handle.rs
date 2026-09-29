//! How a backend makes and reads a concrete heap type.

/// A concrete heap type, as a backend makes and reads it.
///
/// [`TypeHandle`](crate::TypeHandle) implements the trait. The number
/// inside a handle is the backend's own. A backend gives two handles the
/// same number exactly when its engine takes them for the same type, so two
/// handles from one engine compare equal exactly when they name one type.
pub trait RawTypeHandle: Copy {
    /// Makes the handle that the backend numbers `raw`.
    fn from_raw(raw: u64) -> Self;

    /// The backend's number for the handle.
    fn raw(&self) -> u64;
}
