// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An error context, as a typed entry carries it.

use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::internal::{ErrorContextAnyInternal, ErrorContextInternal};
use crate::store::{StoreContext, StoreId};
use crate::types::ValueType;
use crate::value::Val;

use super::error_context_any::ErrorContextAny;

use super::error_context_id::ErrorContextId;

/// An error context (`error-context`), as a typed host function or a
/// typed call carries it. The name is Wasmtime's.
///
/// It is the typed face of [`ErrorContextAny`](crate::ErrorContextAny)
/// and follows the same rules: it has no operation, a lift to the host
/// marks its record host-held so the record stays until the store
/// drops, and a lower into a guest gives the guest a handle of its own.
/// Passed to a call into another store, it fails with the wrong-store
/// cause before the call changes anything, as
/// [`ErrorContextAny`](crate::ErrorContextAny) states. Inside a type of
/// the host's own, it is checked before the call only when that type's
/// [`Lower`](crate::Lower) checks the store of each value it holds, as
/// the derived and the built-in types do; otherwise its lower inside
/// the call fails the call, which poisons the store.
///
/// It lifts and lowers as a typed value, and it converts to and from a
/// [`Val::ErrorContext`](crate::Val::ErrorContext) with
/// [`into_val`](Self::into_val) and [`from_val`](Self::from_val), as
/// Wasmtime's does.
#[derive(Debug)]
pub struct ErrorContext {
    store: StoreId,
    context: ErrorContextId,
}

impl ErrorContext {
    /// Convert this error context into a [`Val`]. The name is
    /// Wasmtime's.
    pub fn into_val(self) -> Val {
        Val::ErrorContext(ErrorContextAny::new(self.store, self.context))
    }

    /// Convert `value` into an error context, in the store `store`
    /// reaches. The name is Wasmtime's.
    ///
    /// Fails with the host-value-mismatch ABI cause when `value` is not
    /// a [`Val::ErrorContext`].
    pub fn from_val<D: 'static>(store: &mut StoreContext<'_, D>, value: &Val) -> Result<Self> {
        let _ = store;
        match value {
            Val::ErrorContext(context) => Ok(Self::new(context.store_id(), context.context())),
            _ => Err(Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: Some(ValueType::ErrorContext),
                cause: AbiCause::HostValueMismatch,
            })),
        }
    }
}

impl ErrorContextInternal for ErrorContext {
    fn new(store: StoreId, context: ErrorContextId) -> Self {
        Self { store, context }
    }

    fn store_id(&self) -> StoreId {
        self.store
    }

    fn context(&self) -> ErrorContextId {
        self.context
    }
}
