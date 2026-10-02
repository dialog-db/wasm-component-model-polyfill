// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The qualified name of an interface within a component package.

use core::fmt;
use core::str::FromStr;

use super::package_name::{PackageName, parse_package};
use super::parse::{IdentifierParseError, split_optional_version};

/// A `namespace:name/iface` interface identifier with an optional
/// semver version.
///
/// An interface identifier is the addressing surface for a component
/// import or export: it pairs a [`PackageName`] with the interface's
/// own name. Two identifiers compare equal if both halves are equal.
///
/// # Textual form
///
/// `InterfaceIdentifier` parses the canonical WIT identifier syntax,
/// where the version is optional and trails the interface:
///
/// ```text
/// wasi:cli/run
/// wasi:cli/run@0.2.0
/// ```
///
/// The parser also accepts the version between the package and the
/// interface (`wasi:cli@0.2.0/run`), a spelling the grammar does not
/// have. Both spellings parse to the same value, which prints in the
/// grammar's trailing form, so the extra spelling never leaves the
/// parser. A version on both sides is rejected as ambiguous.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InterfaceIdentifier {
    package: PackageName,
    name: String,
}

impl InterfaceIdentifier {
    /// Construct an interface identifier from its parts.
    pub fn new(package: PackageName, name: impl Into<String>) -> Self {
        Self {
            package,
            name: name.into(),
        }
    }

    /// The package this interface belongs to.
    pub fn package(&self) -> &PackageName {
        &self.package
    }

    /// The interface's own name (the segment after the slash).
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for InterfaceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}/{}",
            self.package.namespace(),
            self.package.name(),
            self.name
        )?;
        if let Some(version) = self.package.version() {
            write!(f, "@{version}")?;
        }
        Ok(())
    }
}

impl FromStr for InterfaceIdentifier {
    type Err = IdentifierParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (package_part, iface_part) = input
            .split_once('/')
            .ok_or(IdentifierParseError::MissingInterfaceSegment)?;
        let mut package = parse_package(package_part)?;

        let (iface_name, trailing_version) = split_optional_version(iface_part)?;
        if iface_name.is_empty() {
            return Err(IdentifierParseError::EmptySegment);
        }
        if let Some(version) = trailing_version {
            if package.version().is_some() {
                return Err(IdentifierParseError::DuplicateVersion);
            }
            package.set_version(Some(version));
        }

        Ok(Self::new(package, iface_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    #[wcmp_macros::test]
    fn it_parses_an_interface_with_a_package_version() {
        let id: InterfaceIdentifier = "wasi:cli@0.2.0/run".parse().unwrap();
        assert_eq!(id.package().version().unwrap(), &Version::new(0, 2, 0));
        assert_eq!(id.name(), "run");
    }

    #[wcmp_macros::test]
    fn it_normalises_a_trailing_version_onto_the_package() {
        let id: InterfaceIdentifier = "wasi:cli/run@0.2.0".parse().unwrap();
        assert_eq!(id.package().version().unwrap(), &Version::new(0, 2, 0));
        assert_eq!(id.name(), "run");
    }

    #[wcmp_macros::test]
    fn it_rejects_a_double_version() {
        assert!(matches!(
            "wasi:cli@0.2.0/run@0.2.0"
                .parse::<InterfaceIdentifier>()
                .unwrap_err(),
            IdentifierParseError::DuplicateVersion
        ));
    }

    #[wcmp_macros::test]
    fn it_rejects_an_identifier_missing_the_interface_segment() {
        assert!(matches!(
            "wasi:cli".parse::<InterfaceIdentifier>().unwrap_err(),
            IdentifierParseError::MissingInterfaceSegment
        ));
    }

    #[wcmp_macros::test]
    fn it_rejects_an_empty_interface_segment() {
        assert!(matches!(
            "wasi:cli/".parse::<InterfaceIdentifier>().unwrap_err(),
            IdentifierParseError::EmptySegment
        ));
    }

    #[wcmp_macros::test]
    fn it_displays_in_the_trailing_version_form() {
        let id: InterfaceIdentifier = "wasi:cli@0.2.0/run".parse().unwrap();
        assert_eq!(id.to_string(), "wasi:cli/run@0.2.0");

        let unversioned: InterfaceIdentifier = "wasi:cli/run".parse().unwrap();
        assert_eq!(unversioned.to_string(), "wasi:cli/run");
    }
}
