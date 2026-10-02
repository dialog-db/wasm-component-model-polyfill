// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The contract a backend implements, and the bounds it and the host share.

mod backend;
mod backend_module;
mod backend_resumption;
mod backend_store;
mod backend_suspended_call;
mod box_future;
mod host_func;
mod maybe_send;
mod maybe_sync;
mod missing_capability;
mod raw_handle;
mod raw_type_handle;

pub use backend::Backend;
pub use backend_module::BackendModule;
pub use backend_resumption::BackendResumption;
pub use backend_store::BackendStore;
pub use backend_suspended_call::BackendSuspendedCall;
pub use box_future::BoxFuture;
pub use host_func::HostFunc;
pub use maybe_send::MaybeSend;
pub use maybe_sync::MaybeSync;
pub use missing_capability::missing_capability;
pub use raw_handle::RawHandle;
pub use raw_type_handle::RawTypeHandle;
