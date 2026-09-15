//! Identity of a resource type referenced by an `own<T>` or
//! `borrow<T>` handle slot.
//!
//! A resource type is named by a label, the name under which the
//! component imports or exports it, and positioned by an index, the
//! resource table of the component that a handle of this type lives
//! in (one per component instance per resource). The label is what a
//! reader of a component's imports and exports sees; the index is
//! what the canonical ABI uses to find the handle table.

/// The identity of a resource type referenced by a handle.
///
/// Two resource types compare equal when their labels match. The
/// label is whatever name the parser assigned the resource — its
/// declared name in the component's local types, or the import path
/// it was introduced at — and is meaningful only within the
/// component it belongs to. The index does not take part in equality:
/// it positions the resource within one component's resource list and
/// is `None` for a resource type that no concrete component defines.
#[derive(Clone, Debug)]
pub struct ResourceType {
    label: String,
    index: Option<usize>,
}

impl ResourceType {
    /// Construct a resource type identity from a label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            index: None,
        }
    }

    /// Construct a resource type identity from a label and the index
    /// of the resource table it lives in within its component.
    pub fn indexed(label: impl Into<String>, index: usize) -> Self {
        Self {
            label: label.into(),
            index: Some(index),
        }
    }

    /// The resource type's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The index of the resource table the type lives in within its
    /// component, when a concrete instance holds it.
    pub fn index(&self) -> Option<usize> {
        self.index
    }
}

impl PartialEq for ResourceType {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
    }
}

impl Eq for ResourceType {}

impl std::hash::Hash for ResourceType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.label.hash(state);
    }
}
