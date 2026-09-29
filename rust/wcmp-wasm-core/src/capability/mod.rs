//! The capability lexicon, and the set of capabilities a backend declares.

mod capabilities;
#[allow(clippy::module_inception)]
mod capability;

pub use capabilities::Capabilities;
pub use capability::Capability;
