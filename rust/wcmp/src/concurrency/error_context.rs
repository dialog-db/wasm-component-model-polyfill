// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An error context, as a typed entry carries it.

use crate::internal::ErrorContextInternal;

use crate::store::StoreId;

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
/// [`ErrorContextAny`](crate::ErrorContextAny) states. Held in a
/// `ComponentValue` type of the host's own, it is not checked before the
/// call; its lower inside the call fails the call, which poisons the
/// store. It implements [`ComponentValue`](crate::ComponentValue), which
/// converts it to and from a
/// [`Val::ErrorContext`](crate::Val::ErrorContext) and lifts and
/// lowers it as a typed value.
#[derive(Debug)]
pub struct ErrorContext {
    store: StoreId,
    context: ErrorContextId,
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
