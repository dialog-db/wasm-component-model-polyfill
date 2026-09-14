//! The polyfill's parsed-component value.

use core::fmt;
use std::sync::Arc;

use super::component_export::ComponentExport;
use super::component_import::ComponentImport;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::executor::ir::ExecutorIr;
use crate::executor::translate;

/// The 8-byte preamble of a Wasm core module: `\0asm` followed by
/// the core-module version word.
const CORE_MODULE_PREAMBLE: [u8; 8] = [b'\0', b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];

/// A parsed Component Model component.
///
/// `Component` carries the introspectable shape of a component
/// binary — its declared imports and exports, resolved to the
/// polyfill's own data shapes — together with the executor's plan:
/// the runtime-layer modules each `(core module ...)` section
/// compiled to and the orchestration sequence the executor walks at
/// instantiation time. The binary is translated and compiled exactly
/// once, here; instantiation only executes the plan.
///
/// Construct one with [`Component::new`]. Failure to decode the
/// bytes — corrupted preamble, truncated section, a feature the
/// polyfill does not implement — surfaces as a structured
/// [`Error::InvalidComponentBinary`], [`Error::Unsupported`], or, for
/// core-module bytes, [`Error::NotAComponent`].
///
/// [`Error::InvalidComponentBinary`]: crate::Error::InvalidComponentBinary
/// [`Error::Unsupported`]: crate::Error::Unsupported
/// [`Error::NotAComponent`]: crate::Error::NotAComponent
#[derive(Clone)]
pub struct Component {
    /// The declared imports of this component, in the order they
    /// appeared in the binary.
    pub imports: Box<[ComponentImport]>,
    /// The declared exports of this component, in the order they
    /// appeared in the binary.
    pub exports: Box<[ComponentExport]>,
    /// The executor's plan for this component. Shared between
    /// clones because the compiled modules inside it are immutable.
    /// Workspace-internal; not part of the public contract.
    pub ir: Arc<ExecutorIr>,
}

impl Component {
    /// Parse a Component Model binary against an [`Engine`].
    ///
    /// The byte slice is borrowed only for the duration of the
    /// call. The translator runs once, every core module is
    /// compiled against the engine, and the resulting plan lives
    /// inside the returned [`Component`].
    pub fn new(engine: &Engine, bytes: &[u8]) -> Result<Self> {
        if bytes.len() >= CORE_MODULE_PREAMBLE.len()
            && bytes[..CORE_MODULE_PREAMBLE.len()] == CORE_MODULE_PREAMBLE
        {
            return Err(Error::NotAComponent);
        }
        let translation = translate(engine, bytes)?;
        Ok(Self {
            imports: translation.imports,
            exports: translation.exports,
            ir: Arc::new(translation.ir),
        })
    }
}

impl fmt::Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("imports", &self.imports)
            .field("exports", &self.exports)
            .finish_non_exhaustive()
    }
}
