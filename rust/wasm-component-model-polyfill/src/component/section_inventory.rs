//! A summary of which top-level component sections were observed
//! during parsing.

/// A count of the top-level sections a [`Component`] was assembled
/// from.
///
/// The Component Model binary divides a component into a sequence
/// of typed sections — types, imports, exports, core modules,
/// instances, aliases, and so on. The polyfill keeps a count of
/// how many of each it observed at the *outer* level (sections
/// nested inside core modules or sub-components are not counted)
/// so a parsed component can be inspected without having to
/// re-walk the original bytes. Zero counts are valid — a component
/// is not required to declare every kind of section.
///
/// A `SectionInventory` is built once during parsing and is read
/// directly from its public fields thereafter.
///
/// [`Component`]: super::Component
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SectionInventory {
    /// The number of component-type sections declared at the outer
    /// level.
    pub component_types: usize,
    /// The number of core-type sections declared at the outer
    /// level.
    pub core_types: usize,
    /// The number of component-import sections declared at the
    /// outer level.
    pub component_imports: usize,
    /// The number of component-export sections declared at the
    /// outer level.
    pub component_exports: usize,
    /// The number of nested core modules declared at the outer
    /// level.
    pub core_modules: usize,
    /// The number of core-instance sections declared at the outer
    /// level.
    pub core_instances: usize,
    /// The number of component-instance sections declared at the
    /// outer level.
    pub component_instances: usize,
    /// The number of alias sections declared at the outer level.
    pub aliases: usize,
    /// The number of canonical-function sections declared at the
    /// outer level.
    pub canonicals: usize,
    /// The number of component-start sections declared at the
    /// outer level. The synchronous baseline does not implement
    /// component-level start functions; this count is provided so
    /// that a binary that nevertheless declares one round-trips
    /// without surprise.
    pub starts: usize,
    /// The number of nested components declared at the outer
    /// level.
    pub nested_components: usize,
    /// The number of custom sections declared at the outer level.
    pub custom_sections: usize,
}
