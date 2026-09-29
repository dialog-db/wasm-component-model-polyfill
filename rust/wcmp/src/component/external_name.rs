//! The name of a component import or export.

use core::fmt;

use crate::identifier::InterfaceIdentifier;

/// The textual key under which an import or export is declared.
///
/// Component-level imports and exports are addressed by a string. A
/// string that follows the WIT interface-name syntax —
/// `namespace:name[/iface[@version]]` — is parsed into an
/// [`InterfaceIdentifier`]; everything else is preserved verbatim
/// as a [plain](Self::Plain) name.
///
/// Two external names compare equal when they refer to the same
/// underlying name: two `Interface` variants compare equal under the
/// same rules as [`InterfaceIdentifier`], and a `Plain` and an
/// `Interface` are never equal even when their text would round-trip
/// to the same string.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ExternalName {
    /// A WIT interface identifier — `namespace:name/iface[@version]`.
    Interface(InterfaceIdentifier),
    /// An unqualified name that does not parse as an interface
    /// identifier.
    Plain(String),
}

impl ExternalName {
    /// Build an [`ExternalName`] from the raw component-binary
    /// string.
    ///
    /// Strings that parse as a WIT interface identifier produce
    /// [`Self::Interface`]; everything else is captured as
    /// [`Self::Plain`]. Parsing never fails — a non-conforming name
    /// simply lands in the plain variant — because the polyfill
    /// must round-trip every name a component declares, even names
    /// that fall outside the interface-naming convention.
    pub fn from_raw(raw: &str) -> Self {
        match raw.parse::<InterfaceIdentifier>() {
            Ok(id) => ExternalName::Interface(id),
            Err(_) => ExternalName::Plain(raw.to_owned()),
        }
    }
}

impl fmt::Display for ExternalName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalName::Interface(id) => write!(f, "{id}"),
            ExternalName::Plain(s) => f.write_str(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_parses_an_interface_form_into_the_interface_variant() {
        let name = ExternalName::from_raw("wasi:cli/run@0.2.0");
        assert!(matches!(name, ExternalName::Interface(_)));
    }

    #[wcmp_macros::test]
    fn it_falls_back_to_plain_for_a_non_conforming_name() {
        let name = ExternalName::from_raw("hello");
        assert!(matches!(name, ExternalName::Plain(_)));
    }

    #[wcmp_macros::test]
    fn it_falls_back_to_plain_for_a_kebab_case_label() {
        let name = ExternalName::from_raw("hello-world");
        assert!(matches!(name, ExternalName::Plain(_)));
    }

    #[wcmp_macros::test]
    fn it_distinguishes_interface_and_plain_even_when_text_matches() {
        let interface = ExternalName::Interface("wasi:cli/run".parse().unwrap());
        let plain = ExternalName::Plain("wasi:cli/run".to_owned());
        assert_ne!(interface, plain);
    }
}
