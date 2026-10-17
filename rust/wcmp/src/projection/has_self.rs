// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The projection to the whole host data.

use core::marker::PhantomData;

use super::has_data::HasData;

/// The [`HasData`] whose view is the whole host data, `&mut T`, as
/// Wasmtime's `HasSelf` is.
///
/// A host that implements an interface's traits on its own host data
/// registers with this type and the identity function, `|data| data`.
/// No value of the type exists.
pub struct HasSelf<T: ?Sized>(PhantomData<T>);

impl<T: ?Sized + 'static> HasData for HasSelf<T> {
    type Data<'a> = &'a mut T;
}
