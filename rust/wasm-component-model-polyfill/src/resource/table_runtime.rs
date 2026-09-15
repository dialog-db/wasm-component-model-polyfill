//! One component instance's handle table for one resource type.

use super::identity::ResourceTypeId;
use super::table_id::TableId;

/// One resource table of a component instance, as the canonical ABI
/// addresses it: the table itself, the resource type it holds, and
/// whether the instance is the one that defines the resource. The
/// defining instance handles its own resource's reps directly: a
/// borrow lowered into it, or lifted out of it, is the rep itself
/// rather than a table entry.
#[derive(Clone, Copy, Debug)]
pub struct ResourceTableRuntime {
    /// The table the instance keeps for the resource.
    pub table: TableId,
    /// The identity of the resource type the table holds.
    pub type_id: ResourceTypeId,
    /// The resource's index in the component's resource list.
    pub resource_index: usize,
    /// Whether the table's instance defines the resource.
    pub defining: bool,
}
