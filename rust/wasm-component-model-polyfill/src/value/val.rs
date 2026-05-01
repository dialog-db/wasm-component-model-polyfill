//! The polyfill's component-level value enum.

/// A single component-level value passed to or returned from an
/// exported component function.
///
/// `Val` is the polyfill's own value enum; no [`wasm_runtime_layer`]
/// or upstream component-layer type appears in its shape. Equality
/// and hashing are structural.
///
/// Each variant carries the host-readable Rust representation of one
/// shape in [`crate::ValueType`]. Compound variants own their
/// payloads — a `Val::List` carries an owned `Box<[Val]>`, a
/// `Val::Record` carries an owned slice of `(name, value)` pairs, and
/// so on — so a `Val` can be passed across an export call without
/// borrowing into the runtime substrate's memory.
///
/// The handle variants `Val::Own` and `Val::Borrow` are present so
/// the enum is closed for every shape `ValueType` admits, but their
/// canonical-ABI lift and lower are deferred. Constructing or
/// receiving one against an export today surfaces a structured
/// error rather than reaching the underlying handle table.
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
    /// A UTF-8 string.
    String(String),
    /// A homogeneous list of values.
    ///
    /// The element type is carried by the `ValueType::List` the call
    /// site is parameterised by; the payload only carries the values
    /// themselves.
    List(Box<[Val]>),
    /// A record value: an ordered list of named field values.
    ///
    /// Field order matches the corresponding `RecordType.fields()`
    /// order. Field names are duplicated here so a `Val::Record` is
    /// self-describing for diagnostics; the canonical-ABI machinery
    /// indexes by position, not by name.
    Record(Box<[ValField]>),
    /// A heterogeneous, positionally-addressed tuple of values.
    Tuple(Box<[Val]>),
    /// A discriminated case with an optional payload.
    ///
    /// `discriminant` names the active case; `payload` is `Some` iff
    /// the corresponding `VariantCase` declares a payload type.
    Variant {
        /// The active case's name.
        discriminant: String,
        /// The active case's payload, if its declared type is `Some`.
        payload: Option<Box<Val>>,
    },
    /// A tag-only enumeration value, named by the active discriminant.
    Enum(String),
    /// A may-be-absent value.
    Option(Option<Box<Val>>),
    /// A success-or-failure value, each arm with an optional payload.
    Result(core::result::Result<Option<Box<Val>>, Option<Box<Val>>>),
    /// A bit-set whose set members are addressed by name.
    ///
    /// The order of the entries here is unspecified; the canonical-
    /// ABI lower walks the set against the declared `FlagsType` order
    /// and rejects unknown names.
    Flags(Box<[String]>),
    /// An owning handle to a resource (`own<T>`).
    ///
    /// Present so the enum is closed for every shape `ValueType`
    /// admits; the polyfill rejects lift and lower of this variant
    /// today with a structured error.
    Own(ResourceHandle),
    /// A borrow handle to a resource (`borrow<T>`).
    ///
    /// Present so the enum is closed for every shape `ValueType`
    /// admits; the polyfill rejects lift and lower of this variant
    /// today with a structured error.
    Borrow(ResourceHandle),
}

/// One field of a [`Val::Record`].
#[derive(Clone, Debug, PartialEq)]
pub struct ValField {
    /// The field's name. Duplicated from the corresponding
    /// `RecordField` so the value is self-describing for diagnostics.
    pub name: String,
    /// The field's value.
    pub value: Val,
}

/// An opaque, polyfill-typed handle into a resource table.
///
/// Constructing or receiving one is reserved for the resource layer
/// the polyfill introduces with the resource handle work; today the
/// type exists so [`Val::Own`] and [`Val::Borrow`] are constructible
/// in tests that only assert error behaviour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceHandle {
    /// The resource table entry this handle addresses. The handle-
    /// table semantics are deferred; the field is preserved so
    /// downstream work can attach those semantics without reshaping
    /// the public type.
    pub index: u32,
}
