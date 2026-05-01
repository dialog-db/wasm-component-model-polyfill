//! The kind-and-shape an import or export presents at the component
//! boundary.

use super::function_type::FunctionType;
use super::instance_type::InstanceType;
use crate::types::{ResourceType, ValueType};

/// What an import or export of a component is, structurally.
///
/// A component-level import or export can be many things — a
/// function, a typed bag of further exports, an opaque core module,
/// a fresh resource type, and so on. `ExternType` captures the
/// kind alongside whatever shape data is meaningful for that kind.
///
/// Equality is structural: two extern types compare equal when they
/// describe the same kind of thing with the same shape.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ExternType {
    /// A component-level function with a typed signature.
    Function(FunctionType),
    /// A typed instance — a named collection of further exports
    /// (the typical shape of an interface import or export).
    Instance(InstanceType),
    /// A core WebAssembly module imported or exported by the
    /// component as an opaque unit. Its detailed shape is not
    /// modelled by the polyfill at this stage.
    Module,
    /// A nested component imported or exported as an opaque unit.
    /// Its detailed shape is not modelled by the polyfill at this
    /// stage.
    Component,
    /// A fresh resource type — a `(type (sub resource))` import or
    /// resource declaration. The carried [`ResourceType`] names the
    /// resource for round-tripping; the handle-table semantics that
    /// give the resource its runtime identity live in a later layer.
    Resource(ResourceType),
    /// An equality bound on an existing resource type — the
    /// component asserts the import or export refers to the named
    /// resource rather than introducing a fresh one.
    ResourceEquals(ResourceType),
    /// A first-class component value of a known [`ValueType`].
    ///
    /// The Component Model MVP currently treats value imports and
    /// exports as out-of-scope, but the binary format encodes them
    /// and the polyfill preserves the shape so that round-tripping
    /// a binary that contains them is possible. Linking against a
    /// value import is a separate concern this PDD does not address.
    Value(ValueType),
}
