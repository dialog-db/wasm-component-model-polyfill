// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The engine: a backend, and the capabilities it declared.

use core::fmt;
use std::sync::Arc;

use crate::capability::Capabilities;
use crate::contract::Backend;
use crate::internal::EngineInternal;

/// A backend, and the capabilities it declared when the engine was made.
///
/// The engine holds its backend behind dynamic dispatch. No type of the
/// runtime layer names a backend, so one binary can hold engines over two
/// backends at once. The engine is cheap to clone: each clone shares the
/// one backend.
#[derive(Clone)]
pub struct Engine {
    backend: Arc<dyn Backend>,
    capabilities: Capabilities,
}

impl Engine {
    /// An engine over `backend`.
    ///
    /// The engine reads the capabilities of the backend here, once, and
    /// keeps them for its life.
    pub fn with_backend(backend: impl Backend) -> Self {
        let capabilities = backend.capabilities();
        Self {
            backend: Arc::new(backend),
            capabilities,
        }
    }

    /// The capabilities the backend declared when the engine was made.
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// Whether `a` and `b` are clones of one engine.
    pub fn same(a: &Engine, b: &Engine) -> bool {
        Arc::as_ptr(&a.backend).cast::<()>() == Arc::as_ptr(&b.backend).cast::<()>()
    }
}

impl EngineInternal for Engine {
    fn backend(&self) -> &dyn Backend {
        &*self.backend
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}
