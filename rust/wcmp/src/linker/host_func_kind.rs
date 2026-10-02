// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Which of the two registration forms a host function came through.

use std::sync::Arc;

use super::host_func::{ConcurrentHostFuncBody, HostFuncBody};

/// Which of the two registration forms a host function came through,
/// and the body that form carries.
///
/// A synchronous registration is one a developer made through
/// [`LinkerInstance::func_new`] or [`LinkerInstance::func_wrap`]: the
/// call runs its closure to completion on the guest's stack and
/// answers with the result. A concurrent registration is one made
/// through [`LinkerInstance::func_new_concurrent`] or
/// [`LinkerInstance::func_wrap_concurrent`]: the call runs its body
/// only far enough to obtain the future of that one call, and the
/// store owns that future as a host task and polls it until it
/// completes.
///
/// The kind is what the link rule and the trampoline read. A
/// registration records which form it is because the two are not
/// interchangeable at either place: the link rule holds an
/// async-typed import to a concurrent registration and a sync-typed
/// import to a synchronous one, and the trampoline either runs a
/// closure or starts a host task.
///
/// [`LinkerInstance::func_new`]: super::LinkerInstance::func_new
/// [`LinkerInstance::func_wrap`]: super::LinkerInstance::func_wrap
/// [`LinkerInstance::func_new_concurrent`]:
///     super::LinkerInstance::func_new_concurrent
/// [`LinkerInstance::func_wrap_concurrent`]:
///     super::LinkerInstance::func_wrap_concurrent
pub enum HostFuncKind<T: 'static> {
    /// A synchronous registration, carrying the closure one call
    /// runs to completion.
    Synchronous(Arc<HostFuncBody<T>>),
    /// A concurrent registration, carrying the body that produces the
    /// future of one call.
    Concurrent(Arc<ConcurrentHostFuncBody<T>>),
}

/// A kind is cloned by a handle on its body, whatever the host data
/// is: nothing of `T` is held by value, so the clone asks nothing of
/// it.
impl<T: 'static> Clone for HostFuncKind<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Synchronous(call) => Self::Synchronous(call.clone()),
            Self::Concurrent(start) => Self::Concurrent(start.clone()),
        }
    }
}
