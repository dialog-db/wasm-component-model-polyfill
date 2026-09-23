//! Canonical ABI lift and lower for the synchronous baseline.
//!
//! This module realises the [Component Model Canonical ABI rules] for
//! every valtype [`crate::ValueType`] admits, including the handle
//! valtypes `own<T>` and `borrow<T>`, which resolve against the
//! per-store handle tables.
//!
//! The module is organised by concern. [`layout`] computes the
//! size, alignment, flat-slot count, variant-discriminant width, and
//! the parameter-spill layout every other path needs, reading the
//! [`shape`] a compound type computed when it was built. [`lift`] and
//! [`lower`] are the per-valtype recursions that read a
//! [`crate::Val`] out of guest memory and write one back. [`flatten`]
//! is the counterpart for values that travel in flat core slots.
//! [`transcode`] moves a string between two guest memories for an
//! adapter. [`runtime_state`] is where an instantiation deposits the
//! memory, the `cabi_realloc`, and the `post-return` a crossing's
//! options name.
//!
//! [`context`] holds the boundary context, the one object a value
//! crosses through. One is built per crossing from the canon
//! [`options`] of the lift or lower, the component [`instance`], and
//! the task or subtask the crossing counts against, and it selects
//! its [`strategy`] from those options. The instance is where the
//! handle tables of the crossing come from, so a call site hands the
//! context those three things and no table of its own. It is the only object in the
//! polyfill that reads guest memory, writes guest memory, or asks
//! the guest for memory. The two calls it makes into the guest to do
//! so, a `cabi_realloc` and an export's `post-return`, run under a
//! [`boundary_call`], which is what gives a realloc its own task and
//! clears the instance's may-leave flag for the length of either.
//! That flag is the core global the instance's adapters compile
//! against, held here as [`instance_flags`].
//!
//! Workspace-internal: the surface is consumed by `Func::call`, by
//! the host-trampoline path in [`crate::executor::trampoline`], and
//! by the adapter intrinsics in [`crate::executor::intrinsics`]; no
//! `abi` symbol is re-exported from `lib.rs`.
//!
//! [Component Model Canonical ABI rules]:
//!     https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md

pub mod boundary_call;
pub mod call_values;
pub mod context;
pub mod flatten;
pub mod instance;
pub mod instance_flags;
pub mod layout;
mod lift;
mod lower;
pub mod options;
pub mod runtime_state;
pub mod shape;
pub mod strategy;
pub mod strings;
pub mod transcode;

pub use lift::{gate_list, lift, lift_handle, lift_list, lift_string, read_pointer_pair};
pub use lower::{lower, lower_list, lower_list_bytes, lower_str, write_pointer_pair};

/// The list-of-entries type a map is laid out as.
pub fn map_entries_type(map: &crate::types::MapType) -> crate::types::ValueType {
    crate::types::ValueType::List(crate::types::ListType::new(map.entry()))
}

/// A map value as the list of `(key, value)` tuples the canonical ABI
/// lays out.
pub fn map_to_entries(entries: &[(crate::value::Val, crate::value::Val)]) -> crate::value::Val {
    crate::value::Val::List(
        entries
            .iter()
            .map(|(key, value)| crate::value::Val::Tuple(Box::new([key.clone(), value.clone()])))
            .collect(),
    )
}

/// A lifted list of `(key, value)` tuples as a map value.
pub fn entries_to_map(
    list: crate::value::Val,
    ty: &crate::types::ValueType,
    position: crate::error::AbiPosition,
) -> crate::error::Result<crate::value::Val> {
    let malformed = || {
        crate::error::Error::from(crate::error::AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: crate::error::AbiCause::InvalidEncoding {
                message: "a map entry did not lift as a key-value pair".to_owned(),
            },
        })
    };
    let crate::value::Val::List(items) = list else {
        return Err(malformed());
    };
    let mut entries = Vec::with_capacity(items.len());
    for item in items.into_vec() {
        let crate::value::Val::Tuple(pair) = item else {
            return Err(malformed());
        };
        let mut pair = pair.into_vec();
        if pair.len() != 2 {
            return Err(malformed());
        }
        let value = pair.pop().ok_or_else(malformed)?;
        let key = pair.pop().ok_or_else(malformed)?;
        entries.push((key, value));
    }
    Ok(crate::value::Val::Map(entries.into_boxed_slice()))
}
