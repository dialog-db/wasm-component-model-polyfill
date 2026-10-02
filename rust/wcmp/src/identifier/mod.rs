// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Identifier types used to address component imports and exports.
//!
//! Component imports and exports are keyed by qualified names drawn
//! from the WIT identifier syntax — `namespace:name[@semver][/iface]`.
//! The polyfill exposes its own data types for these names so that
//! introspecting a parsed component never surfaces an upstream type
//! to a downstream consumer.
//!
//! Resolution against semver constraints (which candidate satisfies a
//! given import) is the linker's responsibility and is not modelled
//! here; this module is just the addressing surface.

mod interface_identifier;
mod package_name;
mod parse;

pub use interface_identifier::InterfaceIdentifier;
pub use package_name::PackageName;
pub use parse::IdentifierParseError;
