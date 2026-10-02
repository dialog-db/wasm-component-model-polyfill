// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One name of the capability lexicon.

use core::fmt;

/// A Wasm feature above the floor that a backend can declare, or a name the
/// lexicon reserves.
///
/// The floor is Wasm 2.0, which every backend implements. Each capability
/// is a name from a fixed lexicon. Each name for a Wasm feature is the name
/// of the same feature in `wasmparser`'s feature set. A backend declares a
/// capability only where its engine implements the feature faithfully.
///
/// The lexicon is a value, not a set of marker traits: the browser finds
/// its capabilities only at run time, and a host must be able to branch on
/// a capability in generic code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Capability {
    /// Multi-memory: a module with more than one memory.
    MultiMemory,
    /// Memory64: memories and tables addressed with 64-bit numbers.
    Memory64,
    /// Tail calls.
    TailCall,
    /// Exception handling with `exnref`.
    Exceptions,
    /// Typed function references.
    FunctionReferences,
    /// Garbage collection.
    Gc,
    /// Relaxed SIMD.
    RelaxedSimd,
    /// Threads and shared memory.
    Threads,
    /// Stack switching.
    StackSwitching,
    /// Host suspension: a suspending host function can set a resumable call
    /// aside, and the host resumes it later.
    HostSuspension,
    /// Reserved for fuel. No backend declares it.
    Fuel,
    /// Reserved for epoch interruption. No backend declares it.
    EpochInterruption,
    /// Reserved for limits on the growth of memories and tables. No backend
    /// declares it.
    ResourceLimits,
}

impl Capability {
    /// Every name of the lexicon, in the order of the lexicon.
    pub const ALL: [Capability; 13] = [
        Capability::MultiMemory,
        Capability::Memory64,
        Capability::TailCall,
        Capability::Exceptions,
        Capability::FunctionReferences,
        Capability::Gc,
        Capability::RelaxedSimd,
        Capability::Threads,
        Capability::StackSwitching,
        Capability::HostSuspension,
        Capability::Fuel,
        Capability::EpochInterruption,
        Capability::ResourceLimits,
    ];

    /// The name of the capability in the lexicon, such as `multi_memory`.
    pub const fn name(self) -> &'static str {
        match self {
            Capability::MultiMemory => "multi_memory",
            Capability::Memory64 => "memory64",
            Capability::TailCall => "tail_call",
            Capability::Exceptions => "exceptions",
            Capability::FunctionReferences => "function_references",
            Capability::Gc => "gc",
            Capability::RelaxedSimd => "relaxed_simd",
            Capability::Threads => "threads",
            Capability::StackSwitching => "stack_switching",
            Capability::HostSuspension => "host_suspension",
            Capability::Fuel => "fuel",
            Capability::EpochInterruption => "epoch_interruption",
            Capability::ResourceLimits => "resource_limits",
        }
    }

    /// The capability the lexicon names `name`, or `None` where the lexicon
    /// has no such name.
    pub fn from_name(name: &str) -> Option<Capability> {
        Capability::ALL
            .into_iter()
            .find(|capability| capability.name() == name)
    }

    /// Whether the capability is a Wasm feature, named as `wasmparser`
    /// names it. Host suspension and the reserved names are not.
    pub const fn is_wasm_feature(self) -> bool {
        !matches!(
            self,
            Capability::HostSuspension
                | Capability::Fuel
                | Capability::EpochInterruption
                | Capability::ResourceLimits
        )
    }

    /// Whether the lexicon only reserves the name. No backend declares a
    /// reserved name.
    pub const fn is_reserved(self) -> bool {
        matches!(
            self,
            Capability::Fuel | Capability::EpochInterruption | Capability::ResourceLimits
        )
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
