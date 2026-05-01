//! Canonical ABI lift and lower for the synchronous baseline.
//!
//! This module realises the [Component Model Canonical ABI rules] for
//! every baseline valtype the polyfill supports — every type
//! [`crate::ValueType`] admits except `own<T>` and `borrow<T>`. The
//! handle valtypes are accepted by the type-system data, but lifting
//! or lowering them returns
//! [`crate::AbiCause::Unimplemented`] so the public contract stays
//! additive when handles land.
//!
//! The module is organised by concern. [`layout`] computes the
//! size, alignment, flat-slot count, and variant-discriminant width
//! every other path needs. [`lift`] and [`lower`] are the per-valtype
//! recursions that read a [`crate::Val`] out of guest memory or
//! flat core arguments, and write one back. [`context`] carries the
//! per-call state — the runtime-layer memory the value sits in, the
//! optional `cabi_realloc` for heap-allocating types, and the
//! optional `post-return` the caller invokes after a sync lift —
//! that the lift and lower paths read from.
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
