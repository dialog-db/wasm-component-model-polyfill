//! One link between two components of a scenario.

use core::fmt;

use crate::error::{Error, Result};
use crate::linking::Linking;

/// One link of a scenario's wiring: the export of one component that
/// satisfies the import of the same name of another, and when the link
/// is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Whether the runner makes the link at run time or the build
    /// makes it by composition.
    pub linking: Linking,
    /// The component whose import the link satisfies, as the scenario
    /// names it.
    pub importer: String,
    /// The import's name, such as `local:demo/api` for an interface.
    /// The exporter exports an item of the same name.
    pub import: String,
    /// The component whose export satisfies the import.
    pub exporter: String,
}

impl Link {
    /// Check the link against the components it joins: `imports` are
    /// the names of the importer's imports, and `exports` the names of
    /// the exporter's exports. A runner checks each run-time link once
    /// it has parsed both components and before it makes the link, so a
    /// misspelled name in the wiring file fails the run instead of
    /// passing for a stage the scenario reached.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownImport`] when the importer imports nothing under
    /// the link's import name, and [`Error::UnknownExport`] when the
    /// exporter exports nothing under it.
    pub fn check(
        &self,
        imports: impl IntoIterator<Item = impl AsRef<str>>,
        exports: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<()> {
        if !imports.into_iter().any(|name| name.as_ref() == self.import) {
            return Err(Error::UnknownImport {
                link: self.to_string(),
                component: self.importer.clone(),
            });
        }
        if !exports.into_iter().any(|name| name.as_ref() == self.import) {
            return Err(Error::UnknownExport {
                link: self.to_string(),
                component: self.exporter.clone(),
            });
        }
        Ok(())
    }
}

impl fmt::Display for Link {
    /// The link as a line of a wiring file spells it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} {} {}",
            self.linking, self.importer, self.import, self.exporter
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        "run-time importer local:demo/greeter exporter"
            .parse::<crate::wiring::Wiring>()
            .unwrap()
            .links
            .remove(0)
    }

    #[wcmp_macros::test]
    fn it_accepts_a_link_whose_importer_imports_and_exporter_exports_its_name() {
        assert_eq!(
            link().check(
                ["wasi:cli/stdout@0.3.0", "local:demo/greeter"],
                ["local:demo/greeter".to_string()]
            ),
            Ok(())
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_link_whose_name_one_of_its_components_lacks() {
        let line = "run-time importer local:demo/greeter exporter".to_string();
        assert_eq!(
            link().check(["local:demo/greter"], ["local:demo/greeter"]),
            Err(Error::UnknownImport {
                link: line.clone(),
                component: "importer".to_string(),
            })
        );
        assert_eq!(
            link().check(["local:demo/greeter"], ["local:demo/greter"]),
            Err(Error::UnknownExport {
                link: line,
                component: "exporter".to_string(),
            })
        );
    }
}
