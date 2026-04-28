//! The qualified name of a component package.

use core::fmt;
use core::str::FromStr;

use semver::Version;

use super::parse::{IdentifierParseError, split_optional_version};

/// A `namespace:name` package identifier with an optional semver
/// version.
///
/// Two packages compare equal if all three components match exactly.
/// Semver compatibility — deciding whether one version satisfies a
/// constraint declared by another — is the linker's concern, not
/// this type's.
///
/// # Textual form
///
/// `PackageName` parses and prints the canonical WIT package
/// identifier syntax:
///
/// ```text
/// wasi:cli
/// wasi:cli@0.2.0
/// ```
///
/// A leading or trailing `/` is rejected; package strings never
/// carry an interface segment. Use [`InterfaceIdentifier`] for the
/// `namespace:name/iface` form.
///
/// [`InterfaceIdentifier`]: super::InterfaceIdentifier
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PackageName {
    namespace: String,
    name: String,
    version: Option<Version>,
}

impl PackageName {
    /// Construct a package name from its parts.
    pub fn new(
        namespace: impl Into<String>,
        name: impl Into<String>,
        version: Option<Version>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            version,
        }
    }

    /// The package's namespace (the segment before the colon).
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The package's name (the segment after the colon).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The package's optional version.
    pub fn version(&self) -> Option<&Version> {
        self.version.as_ref()
    }

    /// Replace this package's version with the given value.
    ///
    /// Crate-private — used by [`InterfaceIdentifier`]'s parser to
    /// fold a trailing `@version` on the interface segment back onto
    /// the package, where the WIT grammar treats it as belonging.
    ///
    /// [`InterfaceIdentifier`]: super::InterfaceIdentifier
    pub(super) fn set_version(&mut self, version: Option<Version>) {
        self.version = version;
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.name)?;
        if let Some(version) = &self.version {
            write!(f, "@{version}")?;
        }
        Ok(())
    }
}

impl FromStr for PackageName {
    type Err = IdentifierParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.contains('/') {
            return Err(IdentifierParseError::UnexpectedInterfaceSegment);
        }
        parse_package(input)
    }
}

/// Internal helper used by both [`PackageName::from_str`] and
/// [`InterfaceIdentifier`]'s parser. Not exported.
///
/// [`InterfaceIdentifier`]: super::InterfaceIdentifier
pub(super) fn parse_package(input: &str) -> Result<PackageName, IdentifierParseError> {
    let (head, version) = split_optional_version(input)?;
    let (namespace, name) = head
        .split_once(':')
        .ok_or(IdentifierParseError::MissingNamespace)?;
    if namespace.is_empty() || name.is_empty() {
        return Err(IdentifierParseError::EmptySegment);
    }
    Ok(PackageName::new(namespace, name, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_a_package_without_a_version() {
        let pkg: PackageName = "wasi:cli".parse().unwrap();
        assert_eq!(pkg.namespace(), "wasi");
        assert_eq!(pkg.name(), "cli");
        assert!(pkg.version().is_none());
    }

    #[test]
    fn it_parses_a_package_with_a_version() {
        let pkg: PackageName = "wasi:cli@0.2.0".parse().unwrap();
        assert_eq!(pkg.version().unwrap(), &Version::new(0, 2, 0));
    }

    #[test]
    fn it_rejects_a_package_with_an_interface_segment() {
        assert!(matches!(
            "wasi:cli/run".parse::<PackageName>().unwrap_err(),
            IdentifierParseError::UnexpectedInterfaceSegment
        ));
    }

    #[test]
    fn it_rejects_a_package_missing_the_namespace_separator() {
        assert!(matches!(
            "wasicli".parse::<PackageName>().unwrap_err(),
            IdentifierParseError::MissingNamespace
        ));
    }

    #[test]
    fn it_rejects_a_package_with_an_empty_segment() {
        assert!(matches!(
            "wasi:".parse::<PackageName>().unwrap_err(),
            IdentifierParseError::EmptySegment
        ));
    }

    #[test]
    fn it_rejects_a_package_with_an_invalid_semver() {
        assert!(matches!(
            "wasi:cli@nope".parse::<PackageName>().unwrap_err(),
            IdentifierParseError::InvalidVersion(_)
        ));
    }

    #[test]
    fn it_displays_back_to_canonical_form() {
        let pkg: PackageName = "wasi:cli@0.2.0".parse().unwrap();
        assert_eq!(pkg.to_string(), "wasi:cli@0.2.0");

        let unversioned: PackageName = "wasi:cli".parse().unwrap();
        assert_eq!(unversioned.to_string(), "wasi:cli");
    }
}
