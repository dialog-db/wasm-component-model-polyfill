// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a tag's store holds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use web_sys::HtmlElement;

use crate::http_types::{HttpHost, HttpTable};

/// What a tag's store holds: the HTTP resources of its components, and
/// the host node of each element by the id `create` answered, for
/// `emit`.
pub struct ElementHost {
    /// The HTTP resources of the tag's components.
    pub http: HttpTable,
    /// The host node of each element, by the id `create` answered.
    pub nodes: Rc<RefCell<HashMap<u32, HtmlElement>>>,
}

impl HttpHost for ElementHost {
    fn http(&mut self) -> &mut HttpTable {
        &mut self.http
    }
}
