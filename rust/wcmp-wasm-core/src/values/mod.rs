// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The values that cross the boundary of a module.

mod any_ref;
mod cont_ref;
mod exn_ref;
mod extern_ref;
mod i31;
mod val;

pub use any_ref::AnyRef;
pub use cont_ref::ContRef;
pub use exn_ref::ExnRef;
pub use extern_ref::ExternRef;
pub use i31::I31;
pub use val::Val;
