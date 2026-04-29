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
/// resolved to the polyfill's own data shapes. It is the value
/// handed to the linker when the next layer of the polyfill
/// instantiates a component; at this stage it is purely a
/// parsing-and-introspection surface.
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
}

impl Component {
    /// Parse a Component Model binary against an [`Engine`].
    ///
    /// The byte slice is borrowed only for the duration of the
    /// call; the parsed representation is the polyfill's. The
    /// engine is not consulted at this stage but is part of the
    /// signature for forward compatibility with later layers that
    /// will compile against it.
    pub fn new(engine: &Engine, bytes: &[u8]) -> Result<Self> {
        parse::parse_component(engine, bytes)
    }
}
