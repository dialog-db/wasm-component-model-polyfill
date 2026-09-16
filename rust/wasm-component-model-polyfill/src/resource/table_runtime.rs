//! One resource type's view of a component instance's handle table.

use super::identity::ResourceTypeId;
use super::table_id::TableId;

/// One resource type's view of a component instance's handle table,
/// as the canonical ABI addresses it: the table (one per instance,
/// shared by every resource type the instance uses), the resource
/// type, and whether the instance is the one that defines it. The
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
    /// Whether a component defines the resource at all (`true`), or
    /// the host registered it. Read for the trap message.
    pub guest_defined: bool,
}
