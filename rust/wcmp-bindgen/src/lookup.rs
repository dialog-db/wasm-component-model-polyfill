// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The names an item of a world is looked up by, in the `with` option
//! and in the rules of `imports` and `exports`.

use heck::ToUpperCamelCase;
use wit_parser::{InterfaceId, PackageId, Resolve, WorldKey};

use crate::names::to_rust_ident;

/// What is looked up under an interface's names.
#[derive(Clone, Copy)]
pub enum LookupItem<'a> {
    /// The interface itself.
    None,
    /// An item of the interface: a type or a function, by its WIT name.
    Name(&'a str),
    /// The interface itself, by its own names only, not its package's.
    InterfaceNoPop,
}

/// The names `item` of the interface `key` is looked up by, from the
/// most specific to the least, as Wasmtime's `lookup_keys` gives them
/// (`crates/wit-bindgen/src/lib.rs` at `v49.0.0-rc.1`).
///
/// Each name comes with its projection: the Rust path segments from
/// what the name maps to down to the item. For an interface
/// `wasi:io/streams@0.2.0` and an item `input-stream` the names are
/// `wasi:io/streams@0.2.0.input-stream`, `wasi:io/streams.input-stream`,
/// `wasi:io/streams@0.2.0` (projection `InputStream`),
/// `wasi:io/streams` (`InputStream`), `wasi:io@0.2.0`
/// (`streams::InputStream`), `wasi:io` (`streams::InputStream`), and
/// `wasi` (`io::streams::InputStream`). An interface a world names
/// with a plain name is looked up by that name alone.
pub fn lookup_keys(
    resolve: &Resolve,
    key: &WorldKey,
    item: LookupItem<'_>,
) -> Vec<(String, Vec<String>)> {
    let prefix = match key {
        WorldKey::Interface(id) => Prefix::VersionedInterface(*id),
        WorldKey::Name(key) => {
            let name = match item {
                LookupItem::Name(item) => format!("{key}.{item}"),
                LookupItem::None | LookupItem::InterfaceNoPop => key.clone(),
            };
            return vec![(name, Vec::new())];
        }
    };
    let (interface_required, item) = match item {
        LookupItem::None => (false, None),
        LookupItem::Name(item) => (false, Some(item)),
        LookupItem::InterfaceNoPop => (true, None),
    };
    let mut name = Name { prefix, item };
    let mut projection = Vec::new();
    let mut keys = Vec::new();
    loop {
        keys.push((name.key(resolve), projection.clone()));
        if !name.pop(resolve, &mut projection) {
            break;
        }
        if interface_required
            && !matches!(
                name.prefix,
                Prefix::VersionedInterface(_) | Prefix::UnversionedInterface(_)
            )
        {
            break;
        }
    }
    keys
}

/// One name a lookup tries: a prefix, and the item under it.
struct Name<'a> {
    prefix: Prefix,
    item: Option<&'a str>,
}

impl Name<'_> {
    /// The name as a key of the option.
    fn key(&self, resolve: &Resolve) -> String {
        let mut key = self.prefix.key(resolve);
        if let Some(item) = self.item {
            key.push('.');
            key.push_str(item);
        }
        key
    }

    /// Move to the next less specific name, adding to `projection` the
    /// segment the move leaves behind. Answers whether there was one.
    fn pop(&mut self, resolve: &Resolve, projection: &mut Vec<String>) -> bool {
        match (self.item, self.prefix) {
            (Some(_), Prefix::VersionedInterface(id)) => {
                self.prefix = Prefix::UnversionedInterface(id);
                true
            }
            (Some(item), Prefix::UnversionedInterface(id)) => {
                self.prefix = Prefix::VersionedInterface(id);
                self.item = None;
                projection.push(item.to_upper_camel_case());
                true
            }
            (Some(_), _) => false,
            (None, _) => self.prefix.pop(resolve, projection),
        }
    }
}

/// The part of a name that is not the item.
#[derive(Clone, Copy)]
enum Prefix {
    Namespace(PackageId),
    UnversionedPackage(PackageId),
    VersionedPackage(PackageId),
    UnversionedInterface(InterfaceId),
    VersionedInterface(InterfaceId),
}

impl Prefix {
    /// The prefix as a key of the option.
    fn key(self, resolve: &Resolve) -> String {
        match self {
            Prefix::Namespace(id) => resolve.packages[id].name.namespace.clone(),
            Prefix::UnversionedPackage(id) => {
                let mut name = resolve.packages[id].name.clone();
                name.version = None;
                name.to_string()
            }
            Prefix::VersionedPackage(id) => resolve.packages[id].name.to_string(),
            Prefix::UnversionedInterface(id) => {
                let name = interface_id(resolve, id);
                match name.find('@') {
                    Some(at) => name[..at].to_string(),
                    None => name,
                }
            }
            Prefix::VersionedInterface(id) => interface_id(resolve, id),
        }
    }

    /// Move to the next less specific prefix, as [`Name::pop`] does.
    fn pop(&mut self, resolve: &Resolve, projection: &mut Vec<String>) -> bool {
        *self = match *self {
            Prefix::VersionedInterface(id) => Prefix::UnversionedInterface(id),
            Prefix::UnversionedInterface(id) => {
                let interface = &resolve.interfaces[id];
                let (Some(name), Some(package)) = (&interface.name, interface.package) else {
                    return false;
                };
                projection.push(to_rust_ident(name));
                Prefix::VersionedPackage(package)
            }
            Prefix::VersionedPackage(id) => Prefix::UnversionedPackage(id),
            Prefix::UnversionedPackage(id) => {
                projection.push(to_rust_ident(&resolve.packages[id].name.name));
                Prefix::Namespace(id)
            }
            Prefix::Namespace(_) => return false,
        };
        true
    }
}

/// The full name of the interface `id`, as `ns:pkg/iface@version`.
fn interface_id(resolve: &Resolve, id: InterfaceId) -> String {
    resolve.id_of(id).unwrap_or_default()
}
