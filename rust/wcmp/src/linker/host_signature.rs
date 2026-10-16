// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the resolver checks a host function registration against.

use std::sync::Arc;

use crate::component::FunctionType;
use crate::typed::TypeCheckError;
use crate::types::ResourceType;

/// What the resolver checks an import's declared function type
/// against, for one registration.
pub enum HostSignature {
    /// The function type an untyped registration names, compared with
    /// the declared one structurally.
    Declared(FunctionType),
    /// The type check of a typed registration's parameter and result
    /// lists, which walks the declared type with the Rust types.
    Typed(Arc<HostTypeCheck>),
}

/// The type check a typed registration carries: it compares the
/// registration's Rust lists with a declared function type, reading
/// each resource type the type declares as the identity the second
/// argument gives it.
pub type HostTypeCheck = dyn Fn(
        &FunctionType,
        &dyn Fn(ResourceType) -> ResourceType,
    ) -> core::result::Result<(), TypeCheckError>
    + Send
    + Sync
    + 'static;

impl Clone for HostSignature {
    fn clone(&self) -> Self {
        match self {
            Self::Declared(ty) => Self::Declared(ty.clone()),
            Self::Typed(check) => Self::Typed(check.clone()),
        }
    }
}
