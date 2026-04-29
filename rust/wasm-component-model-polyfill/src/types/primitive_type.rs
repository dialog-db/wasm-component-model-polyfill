//! The primitive value types of the Component Model.

/// One of the Component Model's primitive value types.
///
/// Primitives are the leaves of every compound shape. Two primitive
/// types are structurally equal if and only if they are the same
/// variant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PrimitiveType {
    /// `bool`.
    Bool,
    /// Signed 8-bit integer.
    S8,
    /// Unsigned 8-bit integer.
    U8,
    /// Signed 16-bit integer.
    S16,
    /// Unsigned 16-bit integer.
    U16,
    /// Signed 32-bit integer.
    S32,
    /// Unsigned 32-bit integer.
    U32,
    /// Signed 64-bit integer.
    S64,
    /// Unsigned 64-bit integer.
    U64,
    /// IEEE-754 binary32.
    F32,
    /// IEEE-754 binary64.
    F64,
    /// A Unicode scalar value.
    Char,
    /// A UTF-8 string.
    String,
}
