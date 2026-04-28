//! Shared parsing helpers and the error type returned when a
//! `namespace:name[@semver][/iface[@semver]]` string cannot be turned
//! into one of this module's identifier values.

use semver::Version;
use thiserror::Error;

/// Failure parsing an identifier from text.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IdentifierParseError {
    /// A package name is missing the required `namespace:name` form.
    #[error("expected `namespace:name`")]
    MissingNamespace,
    /// An interface identifier is missing the `/iface` segment.
    #[error("expected `/iface` after package name")]
    MissingInterfaceSegment,
    /// A package-only string contained a `/` interface segment.
    #[error("unexpected `/iface` segment in package name")]
    UnexpectedInterfaceSegment,
    /// A semver tag appears on both the package and the interface.
    #[error("version specified twice")]
    DuplicateVersion,
    /// The semver portion failed to parse.
    #[error("invalid semver")]
    InvalidVersion(#[source] semver::Error),
    /// A required name segment was empty.
    #[error("empty identifier segment")]
    EmptySegment,
}

/// Split an identifier string at the optional `@semver` suffix.
///
/// Returns the head segment together with a parsed [`Version`] when
/// a suffix is present.
pub(super) fn split_optional_version(
    input: &str,
) -> Result<(&str, Option<Version>), IdentifierParseError> {
    if let Some((head, tail)) = input.split_once('@') {
        let version = Version::parse(tail).map_err(IdentifierParseError::InvalidVersion)?;
        Ok((head, Some(version)))
    } else {
        Ok((input, None))
    }
}
