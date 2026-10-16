// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the values of a stream or a future are, as its record knows
//! them.

use core::any::TypeId;

use crate::internal::TypeCheckerInternal;
use crate::typed::{ComponentType, TypeChecker};
use crate::types::{ResourceType, ValueType, payloads_match};

/// What the values of a stream or a future are, as its shared record
/// knows them, as Wasmtime's `PayloadType` does: the type a component
/// declares, for a stream or a future a guest created, or the Rust type
/// of the host's, for one the host created.
///
/// A host-created stream has no declared type until a guest receives
/// it. Each place that needs to know what it carries asks instead: a
/// guest's type, through the Rust type's type check, and another Rust
/// type, by comparing the two.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndPayload {
    /// The type a component declares for the values, or `None` for a
    /// stream or a future that carries none.
    Declared(Option<ValueType>),
    /// The Rust type of the values the host created it with.
    Host(HostPayload),
}

/// The Rust type of the values of a stream or a future the host
/// created.
#[derive(Clone, Copy, Debug)]
pub struct HostPayload {
    type_id: TypeId,
    name: &'static str,
    check: fn(&ValueType, &dyn Fn(ResourceType) -> ResourceType) -> bool,
}

impl PartialEq for HostPayload {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
    }
}

impl Eq for HostPayload {}

impl EndPayload {
    /// The values of a stream or a future the host created with values
    /// of the Rust type `T`.
    pub fn host<T: ComponentType + 'static>() -> Self {
        Self::Host(HostPayload {
            type_id: TypeId::of::<T>(),
            name: core::any::type_name::<T>(),
            check: matches_declared::<T>,
        })
    }

    /// The type a component declares for the values, when one did.
    pub fn declared(&self) -> Option<&ValueType> {
        match self {
            Self::Declared(declared) => declared.as_ref(),
            Self::Host(_) => None,
        }
    }

    /// Whether the values are of the type `declared` a component gives
    /// them, whose resources `resources` reads as the identities the
    /// instantiation gave them: the same type, for a stream or a future
    /// a guest created, and one the Rust type's type check accepts, for
    /// the host's.
    pub fn matches(
        &self,
        declared: Option<&ValueType>,
        resources: &dyn Fn(ResourceType) -> ResourceType,
    ) -> bool {
        match (self, declared) {
            (Self::Declared(carried), declared) => {
                payloads_match(carried.as_ref(), declared, resources)
            }
            (Self::Host(host), Some(declared)) => (host.check)(declared, resources),
            (Self::Host(_), None) => false,
        }
    }

    /// Whether the values are values of the Rust type `T`: the type a
    /// component declares passes `T`'s type check, or the host created
    /// the stream or future with `T` itself.
    pub fn carries<T: ComponentType + 'static>(&self) -> bool {
        match self {
            // The record keeps a declared type with its resources'
            // identities, so they are read as they are.
            Self::Declared(Some(declared)) => matches_declared::<T>(declared, &|ty| ty),
            Self::Declared(None) => false,
            Self::Host(host) => host.type_id == TypeId::of::<T>(),
        }
    }

    /// The name of the Rust type, for a stream or a future the host
    /// created.
    pub fn host_name(&self) -> Option<&'static str> {
        match self {
            Self::Host(host) => Some(host.name),
            Self::Declared(_) => None,
        }
    }
}

/// Whether the Rust type `T` stands for `declared`, whose resources
/// `resources` reads as identities.
fn matches_declared<T: ComponentType>(
    declared: &ValueType,
    resources: &dyn Fn(ResourceType) -> ResourceType,
) -> bool {
    T::typecheck(declared, &TypeChecker::new(resources)).is_ok()
}
