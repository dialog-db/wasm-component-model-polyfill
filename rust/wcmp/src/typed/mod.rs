// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed values: Rust values that cross the canonical ABI directly,
//! with Wasmtime's three traits, [`ComponentType`], [`Lift`], and
//! [`Lower`], and its [`ComponentNamedList`] for the parameter and
//! result lists of a function.
//!
//! A typed value never becomes a [`Val`]. Each type lifts and lowers
//! itself through the hidden methods of the traits, which read and
//! write the guest through a [`LiftContext`] or a [`LowerContext`].
//! The contexts batch the guest accesses of a crossing: the elements
//! of a list come out of one read of the list's range, and a lowered
//! list reaches the guest in one write.
//!
//! The type check compares a Rust type with a type a component
//! declares, once, when the host acquires a typed function or links a
//! typed host function. A crossing trusts it and does not compare the
//! types again.
//!
//! [`Val`]: crate::Val

mod component_named_list;
mod component_type;
mod crossing;
mod derived;
mod flat;
mod impls;
mod lift;
mod lift_context;
mod lower;
mod lower_context;
mod named_types;
mod type_check_error;
mod type_checker;

pub use component_named_list::ComponentNamedList;
pub use component_type::ComponentType;
pub use crossing::{
    lift_parameters, lift_results, lift_returned, lower_parameters, lower_results,
    typecheck_function,
};
pub use derived::{
    TypeCheckFn, format_flags, typecheck_enum, typecheck_flags, typecheck_record, typecheck_variant,
};
pub use flat::{align_to, lift_case_flat, lift_case_memory, lower_case_flat, lower_case_memory};
pub use lift::Lift;
pub use lift_context::LiftContext;
pub use lower::Lower;
pub use lower_context::LowerContext;
pub use named_types::NamedTypes;
pub use type_check_error::TypeCheckError;
pub use type_checker::TypeChecker;
#[cfg(test)]
pub use type_checker::type_check_runs;
