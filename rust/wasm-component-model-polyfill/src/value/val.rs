//! The polyfill's component-level value enum.

use crate::concurrency::{ErrorContextAny, FutureAny, StreamAny};
use crate::resource::ResourceHandle;

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
/// The handle variants `Val::Own` and `Val::Borrow` carry a
/// [`crate::ResourceHandle`] whose index names an entry in one table:
/// the store's host table for a handle the host holds, or the guest's
/// own table for a `Val::Borrow` lifted out of a guest, which keeps
/// the guest's index.
///
/// Lowering an `own` into a call moves the host's entry into the
/// guest's table. Lowering a `borrow` lends the host's owning entry
/// to the call, so only a borrow of a handle the host owns can be
/// lowered. The instance that defines the resource then receives
/// only the rep and gains no entry; any other instance receives a
/// borrow entry of its own. Lifting an `own` out of a guest moves the
/// guest's entry into the host table. Lifting a `borrow` out of a
/// guest lends the guest's entry to the call when that entry owns the
/// resource, and lends nothing when the entry is itself a borrow.
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
    /// A `map<K, V>` as its entries, in the order they cross the
    /// boundary. Duplicate keys are carried as they are; a typed
    /// conversion into a Rust map keeps the last value for a key.
    Map(Box<[(Val, Val)]>),
    /// A `list<T, N>`: exactly `N` elements, laid out inline.
    FixedLengthList(Box<[Val]>),
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
    Own(ResourceHandle),
    /// A borrow handle to a resource (`borrow<T>`).
    Borrow(ResourceHandle),
    /// The readable end of a stream (`stream<T>`) the host holds.
    ///
    /// Lowering one into a guest enters the end in the guest's handle
    /// table, after checking that the guest's type carries the
    /// stream's payload type. A typed
    /// [`StreamReader`](crate::StreamReader) crosses as one of these.
    Stream(StreamAny),
    /// The readable end of a future (`future<T>`) the host holds.
    ///
    /// Lowering one into a guest enters the end in the guest's handle
    /// table, after checking that the guest's type carries the
    /// future's payload type. A typed
    /// [`FutureReader`](crate::FutureReader) crosses as one of these.
    Future(FutureAny),
    /// An error context (`error-context`).
    ///
    /// One carries an error context between two components, as the
    /// payload of a stream or a future a copy moves. A lift or a
    /// lower of one between the host and a guest fails with
    /// [`Error::Unsupported`](crate::Error::Unsupported).
    ErrorContext(ErrorContextAny),
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
