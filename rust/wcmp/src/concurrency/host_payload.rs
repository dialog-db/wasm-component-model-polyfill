// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Rust type of the values of a stream or a future the host
//! created.

use core::any::TypeId;

use crate::types::{ResourceType, ValueType};

/// The type check of a host payload's Rust type against a declared
/// type, whose resources the second argument reads as identities.
pub type PayloadCheck = fn(&ValueType, &dyn Fn(ResourceType) -> ResourceType) -> bool;

/// The Rust type of the values of a stream or a future the host
/// created. Two compare equal when the Rust types are the same.
#[derive(Clone, Copy, Debug)]
pub struct HostPayload {
    type_id: TypeId,
    name: &'static str,
    check: PayloadCheck,
}

impl HostPayload {
    /// The payload of values of the Rust type `T`, whose type check is
    /// `check`.
    pub fn of<T: 'static>(check: PayloadCheck) -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            name: core::any::type_name::<T>(),
            check,
        }
    }

    /// Whether the values are of the Rust type `T`.
    pub fn is<T: 'static>(&self) -> bool {
        self.type_id == TypeId::of::<T>()
    }

    /// The name of the Rust type.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Whether the Rust type stands for `declared`.
    pub fn check(
        &self,
        declared: &ValueType,
        resources: &dyn Fn(ResourceType) -> ResourceType,
    ) -> bool {
        (self.check)(declared, resources)
    }
}

impl PartialEq for HostPayload {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
    }
}

impl Eq for HostPayload {}
