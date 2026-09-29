//! The set of capabilities a backend declares.

use core::fmt;

use crate::capability::Capability;
use crate::error::{Error, Result};

/// A set of [`Capability`] names: what a backend declares.
///
/// An engine reads the set from its backend once, when it is made, and
/// keeps it for its life. A reserved name never enters the set a backend
/// declares, because [`Capabilities::with`] leaves it out.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    bits: u16,
}

impl Capabilities {
    /// The empty set: the floor, and nothing above it.
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// The set with `capability` added. A reserved name is left out.
    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        if capability.is_reserved() {
            self
        } else {
            Self {
                bits: self.bits | bit(capability),
            }
        }
    }

    /// The set with `capability` taken out.
    #[must_use]
    pub const fn without(self, capability: Capability) -> Self {
        Self {
            bits: self.bits & !bit(capability),
        }
    }

    /// Whether the set holds `capability`.
    pub const fn contains(self, capability: Capability) -> bool {
        self.bits & bit(capability) != 0
    }

    /// [`Error::Unsupported`] with the name of `capability`, where the set
    /// does not hold it.
    pub fn require(self, capability: Capability) -> Result<()> {
        if self.contains(capability) {
            Ok(())
        } else {
            Err(Error::Unsupported(capability))
        }
    }

    /// The capabilities of the set, in the order of the lexicon.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |capability| self.contains(*capability))
    }
}

/// The bit of `capability` in the set: its place in the lexicon.
const fn bit(capability: Capability) -> u16 {
    1 << capability as u16
}

impl FromIterator<Capability> for Capabilities {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        iter.into_iter()
            .fold(Capabilities::empty(), Capabilities::with)
    }
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_leaves_a_reserved_name_out_of_a_declared_set() {
        let capabilities = Capabilities::empty()
            .with(Capability::Fuel)
            .with(Capability::EpochInterruption)
            .with(Capability::ResourceLimits)
            .with(Capability::Gc);
        assert_eq!(capabilities.iter().collect::<Vec<_>>(), [Capability::Gc]);
    }

    #[wcmp_macros::test]
    fn it_names_a_missing_capability_in_its_error() {
        let capabilities = Capabilities::empty().with(Capability::TailCall);
        assert!(capabilities.require(Capability::TailCall).is_ok());
        let error = capabilities
            .require(Capability::MultiMemory)
            .expect_err("multi_memory is missing");
        assert!(matches!(error, Error::Unsupported(Capability::MultiMemory)));
        assert_eq!(
            error.to_string(),
            "the backend does not support `multi_memory`"
        );
    }

    #[wcmp_macros::test]
    fn it_takes_a_capability_out_again() {
        let capabilities: Capabilities = [Capability::Gc, Capability::Exceptions]
            .into_iter()
            .collect();
        let capabilities = capabilities.without(Capability::Gc);
        assert!(!capabilities.contains(Capability::Gc));
        assert!(capabilities.contains(Capability::Exceptions));
    }
}
