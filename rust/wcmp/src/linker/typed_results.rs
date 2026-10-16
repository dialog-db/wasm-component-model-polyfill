// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A typed registration's result tuple, before it crosses.

use crate::abi::context::BoundaryContext;
use crate::component::FunctionType;
use crate::error::Result;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreData;
use crate::typed::{Lower, lower_results};

/// A typed registration's result tuple, as the trampoline holds it
/// until it crosses into the guest.
pub trait TypedResults<T: 'static>: Send {
    /// Lower the results as the result list of `ty`: into the return
    /// area the guest passed, or into the flat result values of the
    /// call.
    fn lower(
        self: Box<Self>,
        cx: &mut BoundaryContext<'_, StoreData<T>>,
        ty: &FunctionType,
        return_area: Option<usize>,
        flat: &mut [RuntimeVal],
    ) -> Result<()>;
}

impl<T: 'static, R: Lower + 'static> TypedResults<T> for R {
    fn lower(
        self: Box<Self>,
        cx: &mut BoundaryContext<'_, StoreData<T>>,
        ty: &FunctionType,
        return_area: Option<usize>,
        flat: &mut [RuntimeVal],
    ) -> Result<()> {
        lower_results(cx, ty, &*self, return_area, flat)
    }
}
