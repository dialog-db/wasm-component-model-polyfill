// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill's component-level value enum.

use crate::concurrency::{ErrorContextAny, FutureAny, StreamAny};
use crate::resource::ResourceAny;

/// A single component-level value passed to or returned from an
/// exported component function.
///
/// `Val` is the polyfill's own value enum; no runtime-layer type or
/// upstream component-layer type appears in its shape. Equality
/// and hashing are structural.
///
/// Each variant carries the host-readable Rust representation of one
/// shape in [`crate::ValueType`]. Compound variants own their
/// payloads — a `Val::List` carries an owned `Box<[Val]>`, a
/// `Val::Record` carries an owned slice of `(name, value)` pairs, and
/// so on — so a `Val` can be passed across an export call without
/// borrowing into the runtime substrate's memory.
///
/// The handle variant `Val::Resource` carries a
/// [`ResourceAny`], whose index names an entry of the store's host
/// table for its resource type, and which is an `own` or a `borrow`.
///
/// Lowering an `own` into a call moves the host's entry into the
/// guest's table. Lowering a `borrow` lends the host's owning entry
/// to the call. The instance that defines the resource then receives
/// only the rep and gains no entry; any other instance receives a
/// borrow entry of its own. Lifting an `own` out of a guest moves the
/// guest's entry into the host table. Lifting a `borrow` out of a
/// guest lends the guest's entry to the call when that entry owns the
/// resource, and lends nothing when the entry is itself a borrow; the
/// host's handle names a borrow entry of the host table, which goes
/// when the call ends.
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
    /// A 32-bit IEEE-754 float. A NaN lifted from a guest is the
    /// canonical NaN, whatever payload the guest gave it, as the
    /// Component Model says; Wasmtime keeps the payload.
    F32(f32),
    /// A 64-bit IEEE-754 float. A NaN lifted from a guest is the
    /// canonical NaN, as for [`Val::F32`].
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
    /// A handle to a resource (`own<T>` or `borrow<T>`), which
    /// [`ResourceAny::owned`] tells apart.
    Resource(ResourceAny),
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
    /// A lift of one to the host marks its record host-held, so the
    /// record stays until the store drops. Lowering one into a guest
    /// gives the guest a handle of its own. A typed
    /// [`ErrorContext`](crate::ErrorContext) crosses as one of these.
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
