// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The context a typed value lowers through.

use crate::abi::context::BoundaryContext;
use crate::error::{AbiPosition, Error};
use crate::internal::LowerContextInternal;
use crate::types::ValueType;

use super::flat::declared_mismatch;
use super::named_types::NamedTypes;

/// The context one typed value lowers through: the guest's memory, its
/// `cabi_realloc`, the string encoding of the call, and the slot a
/// failure is reported at.
///
/// It is public so that a derive in another crate can name it in the
/// methods it writes, and those methods only hand it on to the fields
/// they lower. Everything it does is the polyfill's own. It writes a
/// lowered list to the guest once, after the host has built the list's
/// bytes, and calls `cabi_realloc` in the order the Canonical ABI
/// gives.
pub struct LowerContext<'c, 'a, D: 'static> {
    cx: &'c mut BoundaryContext<'a, D>,
    position: AbiPosition,
    /// The list the lowering starts at, when it starts at a function's
    /// parameters or results. The tuple that is the list takes it.
    named: Option<NamedTypes<'c>>,
}

impl<D: 'static> LowerContext<'_, '_, D> {
    /// The failure of a crossing whose declared type `ty` is not the
    /// shape the Rust value stands for, at the slot the crossing
    /// reports at. The type check compares the two when the host
    /// acquires the function, so only a value paired with a type of
    /// the caller's own reaches it.
    pub fn mismatch(&self, ty: &ValueType) -> Error {
        declared_mismatch(ty, self.position)
    }
}

impl<'c, 'a, D: 'static> LowerContextInternal<'c, 'a, D> for LowerContext<'c, 'a, D> {
    fn new(cx: &'c mut BoundaryContext<'a, D>, position: AbiPosition) -> Self {
        Self {
            cx,
            position,
            named: None,
        }
    }

    fn named(cx: &'c mut BoundaryContext<'a, D>, named: NamedTypes<'c>) -> Self {
        Self {
            cx,
            position: named.position(0),
            named: Some(named),
        }
    }

    fn boundary(&mut self) -> &mut BoundaryContext<'a, D> {
        self.cx
    }

    fn position(&self) -> AbiPosition {
        self.position
    }

    fn set_position(&mut self, position: AbiPosition) {
        self.position = position;
    }

    fn take_named(&mut self) -> Option<NamedTypes<'c>> {
        self.named.take()
    }
}
