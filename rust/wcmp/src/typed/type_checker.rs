// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a type check knows beyond the two types it compares.

use core::cell::Cell;

use crate::internal::TypeCheckerInternal;
use crate::types::ResourceType;

use super::named_types::NamedTypes;

/// What one run of the type check knows beyond the two types it
/// compares: the identity behind each resource type the component
/// declares.
///
/// A component declares its resources by their position in it, and the
/// identity behind one is the host registration that satisfies its
/// import, or the instance that defines it. A typed resource handle
/// compares that identity with its own.
pub struct TypeChecker<'a> {
    resources: &'a dyn Fn(ResourceType) -> ResourceType,
    /// The list the check starts at, when it starts at a function's
    /// parameters or results rather than at one value. The tuple that
    /// is the list takes it.
    named: Cell<Option<NamedTypes<'a>>>,
}

impl TypeChecker<'_> {
    /// The identity behind `declared`, a resource type the component
    /// declares.
    pub fn resource(&self, declared: ResourceType) -> ResourceType {
        (self.resources)(declared)
    }
}

impl<'a> TypeCheckerInternal<'a> for TypeChecker<'a> {
    fn new(resources: &'a dyn Fn(ResourceType) -> ResourceType) -> Self {
        #[cfg(test)]
        RUNS.with(|runs| runs.set(runs.get() + 1));
        Self {
            resources,
            named: Cell::new(None),
        }
    }

    fn set_named(&self, named: NamedTypes<'a>) {
        self.named.set(Some(named));
    }

    fn take_named(&self) -> Option<NamedTypes<'a>> {
        self.named.take()
    }
}

#[cfg(test)]
thread_local! {
    /// How many type checks this thread has started, which a test of
    /// when the check runs counts.
    static RUNS: Cell<usize> = const { Cell::new(0) };
}

/// How many type checks this thread has started.
#[cfg(test)]
pub fn type_check_runs() -> usize {
    RUNS.with(Cell::get)
}
