// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A module as the Wasmi backend compiled it.

use core::any::Any;
use std::collections::HashMap;

use wasmparser::{Parser, Payload, TypeRef};
use wcmp_wasm_core::backend::BackendModule;
use wcmp_wasm_core::{ExportType, ImportType};

use crate::convert;

/// A module that Wasmi compiled, and the description of its boundary.
///
/// The backend describes the boundary once, when it compiles the module.
/// The description covers the imports and the exports, and nothing inside
/// the module, each in the order the module declares them.
///
/// Wasmi orders both in its own way. It takes the imports of an
/// instantiation grouped by kind: the functions, then the tables, the
/// memories, and the globals, each group in the order the module declares
/// it. It keeps the exports by name. So the backend reads the order of each
/// from the sections of the module, and puts the externs of an
/// instantiation into Wasmi's order.
pub struct WasmiModule {
    module: wasmi::Module,
    imports: Vec<ImportType>,
    link_order: Vec<usize>,
    exports: Vec<ExportType>,
}

impl WasmiModule {
    /// The module `module`, which Wasmi compiled from `bytes`.
    pub fn new(module: wasmi::Module, bytes: &[u8]) -> Self {
        let wasmi_imports = module
            .imports()
            .map(|import| {
                ImportType::new(
                    import.module(),
                    import.name(),
                    convert::extern_type(import.ty()),
                )
            })
            .collect::<Vec<_>>();
        let link_order = link_order(bytes, &wasmi_imports);
        let mut imports = wasmi_imports.clone();
        for (import, declared) in wasmi_imports.into_iter().zip(&link_order) {
            imports[*declared] = import;
        }

        let order = export_order(bytes);
        let mut exports = module
            .exports()
            .map(|export| ExportType::new(export.name(), convert::extern_type(export.ty())))
            .collect::<Vec<_>>();
        exports.sort_by_key(|export| order.get(export.name()).copied().unwrap_or(usize::MAX));
        Self {
            module,
            imports,
            link_order,
            exports,
        }
    }

    /// The module as Wasmi compiled it.
    pub fn module(&self) -> &wasmi::Module {
        &self.module
    }

    /// For each import in the order Wasmi takes them, its place in the
    /// order the module declares them.
    pub fn link_order(&self) -> &[usize] {
        &self.link_order
    }
}

impl BackendModule for WasmiModule {
    fn imports(&self) -> &[ImportType] {
        &self.imports
    }

    fn exports(&self) -> &[ExportType] {
        &self.exports
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// For each of `wasmi_imports`, the imports of the module `bytes` in the
/// order Wasmi takes them, its place in the order the module declares
/// them.
///
/// Wasmi groups the imports by kind, and keeps the order of the module
/// inside each group, so a stable sort of the declared imports by kind is
/// Wasmi's order. The backend checks each name against Wasmi's. Wasmi
/// validated the module before the backend reads it, so the section reads
/// without an error; where the reading disagrees with Wasmi anyway, the
/// order is Wasmi's own.
fn link_order(bytes: &[u8], wasmi_imports: &[ImportType]) -> Vec<usize> {
    let declared = declared_imports(bytes);
    let mut order = (0..declared.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| declared[*index].2);
    let agrees = order.len() == wasmi_imports.len()
        && order.iter().zip(wasmi_imports).all(|(index, import)| {
            let (module, name, _) = &declared[*index];
            (module.as_str(), name.as_str()) == (import.module(), import.name())
        });
    if agrees {
        order
    } else {
        (0..wasmi_imports.len()).collect()
    }
}

/// The imports of the module `bytes`, in the order the module declares
/// them: the module name, the item name, and the place of the kind in
/// Wasmi's order.
fn declared_imports(bytes: &[u8]) -> Vec<(String, String, u8)> {
    let Some(reader) = Parser::new(0)
        .parse_all(bytes)
        .find_map(|payload| match payload {
            Ok(Payload::ImportSection(reader)) => Some(reader),
            _ => None,
        })
    else {
        return Vec::new();
    };
    reader
        .into_imports()
        .filter_map(Result::ok)
        .map(|import| {
            let kind = match import.ty {
                TypeRef::Func(_) | TypeRef::FuncExact(_) => 0,
                TypeRef::Table(_) => 1,
                TypeRef::Memory(_) => 2,
                TypeRef::Global(_) => 3,
                TypeRef::Tag(_) => 4,
            };
            (import.module.to_string(), import.name.to_string(), kind)
        })
        .collect()
}

/// The place of each export of the module `bytes` in its export section.
///
/// An export the reading misses keeps its place in the description, after
/// the others.
fn export_order(bytes: &[u8]) -> HashMap<&str, usize> {
    Parser::new(0)
        .parse_all(bytes)
        .find_map(|payload| match payload {
            Ok(Payload::ExportSection(reader)) => Some(reader),
            _ => None,
        })
        .map(|reader| {
            reader
                .into_iter()
                .filter_map(Result::ok)
                .enumerate()
                .map(|(index, export)| (export.name, index))
                .collect()
        })
        .unwrap_or_default()
}
