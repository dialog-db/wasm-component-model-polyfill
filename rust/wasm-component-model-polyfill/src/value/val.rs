//! The polyfill's component-level value enum.

/// A single component-level value passed to or returned from an
/// exported component function.
///
/// `Val` is the polyfill's own value enum; no [`wasm_runtime_layer`]
/// or upstream component-layer type appears in its shape. Equality
/// and hashing are structural.
///
/// At present the enum carries only the primitive valtypes — every
/// type whose canonical ABI lift and lower is direct passthrough
/// through the underlying core function and does not touch component
/// memory. Compound-valtype variants (records, variants, lists,
/// options, results, tuples, flags, enums, strings, and resource
/// handles) are added additively when their canonical-ABI work
/// lands.
///
/// [`wasm_runtime_layer`]: https://docs.rs/wasm_runtime_layer
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Val {
    /// A `bool`.
    Bool(bool),
    /// A signed 8-bit integer.
    S8(i8),
    /// An unsigned 8-bit integer.
    U8(u8),
    /// A signed 16-bit integer.
    S16(i16),
    /// An unsigned 16-bit integer.
    U16(u16),
    /// A signed 32-bit integer.
    S32(i32),
    /// An unsigned 32-bit integer.
    U32(u32),
    /// A signed 64-bit integer.
    S64(i64),
    /// An unsigned 64-bit integer.
    U64(u64),
    /// A 32-bit IEEE-754 float.
    F32(f32),
    /// A 64-bit IEEE-754 float.
    F64(f64),
    /// A Unicode scalar value.
    Char(char),
}
