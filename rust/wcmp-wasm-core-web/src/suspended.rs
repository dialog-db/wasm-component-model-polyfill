// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A resumable call of the browser backend that waits.

use core::any::Any;
use std::rc::Rc;

use wcmp_wasm_core::backend::BackendSuspendedCall;

use crate::flight::Flight;
use crate::returns::Returns;

/// A resumable call that waits in a suspending host function: its flight,
/// whose stack JavaScript Promise Integration keeps, and how the host
/// reads the results of the call at its end.
///
/// The handle holds no store. Where it drops, the flight drops with it,
/// and nothing can resume the stack any more.
pub struct WebSuspendedCall {
    pub flight: Rc<Flight>,
    pub returns: Returns,
}

impl BackendSuspendedCall for WebSuspendedCall {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}
