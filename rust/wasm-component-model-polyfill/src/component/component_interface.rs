//! The polyfill's parsed-component value.

use super::component_export::ComponentExport;
use super::component_import::ComponentImport;
use super::parse;
use crate::engine::Engine;
use crate::error::Result;

/// A parsed Component Model component.
///
/// `Component` carries the introspectable shape of a component
/// binary — its declared imports and exports, with each side
/// resolved to the polyfill's own data shapes — together with the
/// executor's IR (the runtime-layer modules each `(core module ...)`
/// section produced and the orchestration IR the executor walks at
/// instantiation time). It is the value handed to the linker when
/// the polyfill instantiates a component.
///
/// Construct one with [`Component::new`]. Failure to decode the
/// bytes — corrupted preamble, truncated section, an encoding the
/// polyfill does not yet implement — surfaces as a structured
/// [`Error::InvalidComponentBinary`] (or, for core-module bytes,
/// [`Error::NotAComponent`]).
///
/// [`Error::InvalidComponentBinary`]: crate::Error::InvalidComponentBinary
/// [`Error::NotAComponent`]: crate::Error::NotAComponent
#[derive(Clone, Debug)]
pub struct Component {
    /// The declared imports of this component, in the order they
    /// appeared in the binary.
    pub imports: Box<[ComponentImport]>,
    /// The declared exports of this component, in the order they
    /// appeared in the binary.
    pub exports: Box<[ComponentExport]>,
    /// The component's original bytes, retained so the executor's
    /// IR can be built lazily at instantiation time. Workspace-
    /// internal — the field is `pub` because intra-crate items
    /// follow plain `pub`, but the bytes are not part of the
    /// polyfill's public contract.
    pub bytes: Box<[u8]>,
}

impl Component {
    /// Parse a Component Model binary against an [`Engine`].
    ///
    /// The byte slice is borrowed only for the duration of the
    /// call; the parsed representation is the polyfill's, and the
    /// executor's IR (with runtime-layer modules pre-built) lives
    /// inside the returned [`Component`].
    pub fn new(engine: &Engine, bytes: &[u8]) -> Result<Self> {
        parse::parse_component(engine, bytes)
    }
}
