// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An error context, as an untyped value carries it.

use crate::internal::ErrorContextAnyInternal;

use crate::store::StoreId;

use super::error_context_id::ErrorContextId;

/// An error context, as [`Val::ErrorContext`](crate::Val::ErrorContext)
/// carries it. The name is Wasmtime's.
///
/// It names one error-context record of the store: the debug message
/// a guest gave `error-context.new`. It has no operation, as in
/// Wasmtime: the host cannot read the debug message, create an error
/// context, or drop one. The host receives one from a guest, holds
/// it, and passes it on to a guest, where the guest's own
/// `error-context.debug-message` reads the message.
///
/// A lift to the host marks the record host-held. The host cannot
/// drop an error context, so a host-held record stays until the store
/// drops, however many guest handles drop in the meantime. A lower
/// from the host gives the guest a handle of its own and adds one to
/// the record's count of handles. Wasmtime adds no count there, so a
/// guest drop in Wasmtime can free a record the host still holds; the
/// polyfill keeps the value alive while anything refers to it, as the
/// Component Model's reference does.
///
/// The value names its record by an index and a generation, and
/// cloning it copies the name, not the record. It belongs to the store
/// it was lifted in, and carries that store's identity. A call into
/// another store that passes it fails with
/// [`InstantiationError::WrongStore`](crate::InstantiationError::WrongStore)
/// before the call changes anything, so that store stays usable. A
/// host function or a host stream that hands it to a guest of another
/// store fails the guest's call or built-in with the same cause, as
/// any failure of the host's there does. Wasmtime checks neither, and
/// hands the guest whatever record of that store has the same index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorContextAny {
    store: StoreId,
    context: ErrorContextId,
}

impl ErrorContextAnyInternal for ErrorContextAny {
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
