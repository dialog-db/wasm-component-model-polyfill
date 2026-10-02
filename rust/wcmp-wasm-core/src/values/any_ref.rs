// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An internal reference.

use crate::capability::Capability;
use crate::checks;
use crate::error::Result;
use crate::internal::{StoreContextInternal, StoreContextMutInternal};
use crate::store::{AsContext, AsContextMut};
use crate::values::I31;

handle! {
    /// An internal reference: an `anyref`, `eqref`, `i31ref`, `structref`,
    /// or `arrayref`, or a reference to a concrete struct or array type.
    ///
    /// The host can read the integer of an `i31ref`. A GC object is opaque:
    /// the host can hold it, test it for null (a null is `None` in a
    /// [`Val`](crate::Val)), and give it back to a guest of the same store,
    /// and nothing more. The browser sets this limit, because the JavaScript
    /// API makes a GC object opaque.
    AnyRef
}

impl AnyRef {
    /// The `i31ref` of `value` in `store`.
    ///
    /// The backend must declare [`gc`](Capability::Gc). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn from_i31(mut store: impl AsContextMut, value: I31) -> Result<Self> {
        let mut store = store.as_context_mut();
        store.engine().capabilities().require(Capability::Gc)?;
        store.backend_mut().any_ref_from_i31(value)
    }

    /// The integer of the reference, or `None` where it is not an `i31ref`.
    ///
    /// The backend must declare [`gc`](Capability::Gc). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn as_i31(&self, store: impl AsContext) -> Result<Option<I31>> {
        let store = store.as_context();
        store.engine().capabilities().require(Capability::Gc)?;
        let backend = store.backend();
        checks::same_store(backend, *self)?;
        backend.any_ref_as_i31(*self)
    }
}
