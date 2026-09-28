//! When a link between two components is made.

use core::fmt;
use core::str::FromStr;

use crate::error::Error;

/// When a link between two components of a scenario is made: by the
/// runner, at run time, or by the build, which composes the components
/// into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Linking {
    /// The runner instantiates the exporting component first and gives
    /// its instance's exports to the importing component through the
    /// subject's linker.
    RunTime,
    /// The build composes the components into one component, which the
    /// subjects run like a scenario with one component.
    Composition,
}

impl Linking {
    /// Every kind of link.
    pub const ALL: [Linking; 2] = [Linking::RunTime, Linking::Composition];

    /// The kind's name, as a wiring file spells it.
    pub fn name(self) -> &'static str {
        match self {
            Linking::RunTime => "run-time",
            Linking::Composition => "composition",
        }
    }
}

impl fmt::Display for Linking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Linking {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Linking::ALL
            .into_iter()
            .find(|linking| linking.name() == text)
            .ok_or_else(|| Error::UnknownLinking(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_back_every_kind_it_names() {
        for linking in Linking::ALL {
            assert_eq!(linking.to_string().parse::<Linking>(), Ok(linking));
        }
        assert_eq!(
            "runtime".parse::<Linking>(),
            Err(Error::UnknownLinking("runtime".to_string()))
        );
    }
}
