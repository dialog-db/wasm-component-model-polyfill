//! The structural shape of an instance type — a typed bag of
//! exports.

use super::extern_type::ExternType;

/// An instance type: a typed collection of named exports.
///
/// Component imports and exports keyed by an interface identifier
/// usually carry an instance type — an interface's declarations
/// (its functions, types, resources) are exposed as the exports of
/// an instance.
///
/// Two instance types are structurally equal when their items
/// appear in the same order and each pair of corresponding names
/// and extern types is itself structurally equal.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstanceType {
    /// The instance's exported items, in declaration order.
    pub items: Vec<InstanceItem>,
}

/// A single export of an [`InstanceType`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstanceItem {
    /// The item's name as declared by the instance.
    pub name: String,
    /// The item's extern type.
    pub ty: ExternType,
}
