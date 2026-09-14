//! Canonical ABI lift and lower for the synchronous baseline.
//!
//! This module realises the [Component Model Canonical ABI rules] for
//! every valtype [`crate::ValueType`] admits, including the handle
//! valtypes `own<T>` and `borrow<T>`, which resolve against the
//! per-store handle tables.
//!
//! The module is organised by concern. [`layout`] computes the
//! size, alignment, flat-slot count, variant-discriminant width, and
//! the parameter-spill layout every other path needs. [`lift`] and
//! [`lower`] are the per-valtype recursions that read a
//! [`crate::Val`] out of guest memory and write one back. [`flatten`]
//! is the counterpart for values that travel in flat core slots.
//! [`context`] carries the per-call state — the runtime-layer memory
//! the value sits in and the optional `cabi_realloc` for
//! heap-allocating types — that the lift and lower paths read from.
//!
//! Workspace-internal: the surface is consumed by `Func::call` and
//! by the host-trampoline path in [`crate::executor::instantiate`];
//! no `abi` symbol is re-exported from `lib.rs`.
//!
//! [Component Model Canonical ABI rules]:
//!     https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md

pub mod context;
pub mod flatten;
pub mod layout;
mod lift;
mod lower;

pub use lift::{lift, lift_handle};
pub use lower::lower;
