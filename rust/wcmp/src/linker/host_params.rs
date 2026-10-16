// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the arguments of a host function registration come out of the
//! guest.

use core::any::Any;
use std::sync::Arc;

use crate::abi::context::BoundaryContext;
use crate::component::FunctionType;
use crate::error::Result;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreData;

/// How a registration's arguments come out of the guest.
pub enum HostParams<T: 'static> {
    /// One [`Val`] per parameter, for an untyped registration.
    Vals,
    /// The registration's Rust parameter tuple, lifted straight out of
    /// the guest by this function, for a typed one.
    Typed(Arc<LiftParams<T>>),
}

/// The lift of a typed registration's parameter tuple out of the core
/// arguments of one call, from the cursor: the flat values, or, when
/// the flag is set, the address of the spilled tuple.
pub type LiftParams<T> = dyn for<'a> Fn(
        &mut BoundaryContext<'a, StoreData<T>>,
        &FunctionType,
        &[RuntimeVal],
        &mut usize,
        bool,
    ) -> Result<Box<dyn Any + Send>>
    + Send
    + Sync
    + 'static;

impl<T: 'static> Clone for HostParams<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Vals => Self::Vals,
            Self::Typed(lift) => Self::Typed(lift.clone()),
        }
    }
}
