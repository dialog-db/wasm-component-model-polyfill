//! The errors of the runtime layer, and the kinds of trap.

#[allow(clippy::module_inception)]
mod error;
mod trap_kind;

pub use error::{Error, Result};
pub use trap_kind::TrapKind;
