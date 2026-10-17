// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The trait of any accessor, whatever its projection.

use crate::projection::HasData;

use super::accessor::Accessor;

/// Anything that gives an [`Accessor`], whatever its projection, as
/// Wasmtime's `AsAccessor` is.
///
/// The calls that reach the store from inside a poll take one:
/// [`TypedFunc::call_concurrent`], [`Func::call_concurrent`], and the
/// `close_with` and `guard` of the stream and future readers. So a
/// host method handed an `&Accessor<T, D>` for a projection `D` of its
/// own calls each of them with that accessor, and so does a
/// `run_concurrent` closure with the accessor it is handed. An
/// [`Accessor`] implements the trait, and so does a reference to
/// anything that does.
///
/// [`TypedFunc::call_concurrent`]: crate::TypedFunc::call_concurrent
/// [`Func::call_concurrent`]: crate::Func::call_concurrent
pub trait AsAccessor {
    /// The store's host data.
    type Data: 'static;

    /// The accessor's projection.
    type AccessorData: HasData + ?Sized;

    /// The accessor.
    fn as_accessor(&self) -> &Accessor<Self::Data, Self::AccessorData>;
}

impl<A: AsAccessor + ?Sized> AsAccessor for &A {
    type Data = A::Data;
    type AccessorData = A::AccessorData;

    fn as_accessor(&self) -> &Accessor<Self::Data, Self::AccessorData> {
        A::as_accessor(self)
    }
}

impl<T: 'static, D: HasData + ?Sized> AsAccessor for Accessor<T, D> {
    type Data = T;
    type AccessorData = D;

    fn as_accessor(&self) -> &Accessor<T, D> {
        self
    }
}
