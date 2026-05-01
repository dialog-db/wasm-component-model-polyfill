//! Component-level runtime values.
//!
//! Where [`crate::types`] describes the *shape* a value occupies in
//! the component type system, this module describes the values
//! themselves — the data that travels through a function export call
//! across the polyfill's public API.
//!
//! At present only the primitive valtypes are represented; compound
//! valtype variants (records, variants, lists, options, results,
//! tuples, flags, enums, strings, and resource handles) are out of
//! scope for the export-invocation surface introduced here. Later
//! work extends this enum additively as the canonical-ABI work for
//! compound types lands.

mod val;

pub use val::Val;
