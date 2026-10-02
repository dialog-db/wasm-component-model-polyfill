// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The key an export navigator lookup accepts.
//!
//! [`InstanceExports::instance`] and [`ExportInstance::instance`] take
//! any value that implements [`ExportLookup`], so a caller can name an
//! instance-typed export with a plain string, with a parsed
//! [`InterfaceIdentifier`], or with an [`ExternalName`] taken from
//! [`Component::exports`]. Wasmtime's `ExportLookup` trait plays the
//! same role for its `get_*` accessors, and this trait keeps the name.
//!
//! [`InstanceExports::instance`]: super::InstanceExports::instance
//! [`ExportInstance::instance`]: super::ExportInstance::instance
//! [`Component::exports`]: crate::Component::exports

use crate::component::ExternalName;
use crate::identifier::InterfaceIdentifier;

/// A name that addresses one export of an instance.
///
/// A string is read as the raw name the component declares: a string
/// in WIT interface-name syntax (`namespace:package/interface`, with
/// an optional version) names the interface-named export, and any
/// other string names the plain-named one. An [`InterfaceIdentifier`]
/// names the interface-named export it parses to. The comparison is
/// structural on both sides, so `"a:b/c@1.0.0"` and the identifier it
/// parses to address the same export.
pub trait ExportLookup {
    /// The name in the polyfill's structured form.
    fn external_name(&self) -> ExternalName;
}

impl<T: ExportLookup + ?Sized> ExportLookup for &T {
    fn external_name(&self) -> ExternalName {
        (**self).external_name()
    }
}

impl ExportLookup for str {
    fn external_name(&self) -> ExternalName {
        ExternalName::from_raw(self)
    }
}

impl ExportLookup for String {
    fn external_name(&self) -> ExternalName {
        ExternalName::from_raw(self)
    }
}

impl ExportLookup for InterfaceIdentifier {
    fn external_name(&self) -> ExternalName {
        ExternalName::Interface(self.clone())
    }
}

impl ExportLookup for ExternalName {
    fn external_name(&self) -> ExternalName {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_an_interface_shaped_string_as_the_interface_name() {
        let from_string = "test:guest/foo@1.2.3".external_name();
        let from_identifier = "test:guest/foo@1.2.3"
            .parse::<InterfaceIdentifier>()
            .unwrap()
            .external_name();
        assert_eq!(from_string, from_identifier);
        assert!(matches!(from_string, ExternalName::Interface(_)));
    }

    #[wcmp_macros::test]
    fn it_reads_any_other_string_as_a_plain_name() {
        assert_eq!("a".external_name(), ExternalName::Plain("a".to_owned()));
        assert_eq!(
            String::from("my-label").external_name(),
            ExternalName::Plain("my-label".to_owned())
        );
    }

    #[wcmp_macros::test]
    fn it_passes_an_external_name_through() {
        let name = ExternalName::Plain("i".to_owned());
        let borrowed: &ExternalName = &name;
        assert_eq!(borrowed.external_name(), name);
        assert_eq!(name.external_name(), name);
    }
}
