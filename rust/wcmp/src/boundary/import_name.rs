// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use std::sync::Arc;

/// The name the boundary counters count one host import under, made
/// once when the import's trampoline is built. Without the
/// `boundary-counters` feature it holds nothing, and making it costs
/// nothing.
#[derive(Clone, Default)]
pub struct ImportName {
    #[cfg(feature = "boundary-counters")]
    name: Option<Arc<str>>,
}

impl ImportName {
    /// The name `name` gives, which runs only with the feature.
    #[inline]
    pub fn new(name: impl FnOnce() -> String) -> Self {
        #[cfg(feature = "boundary-counters")]
        {
            ImportName {
                name: Some(Arc::from(name())),
            }
        }
        #[cfg(not(feature = "boundary-counters"))]
        {
            let _ = name;
            ImportName {}
        }
    }

    /// The name, or `None` for no import.
    #[cfg(feature = "boundary-counters")]
    pub fn get(&self) -> Option<&Arc<str>> {
        self.name.as_ref()
    }
}
