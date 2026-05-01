//! The cross-target executor driver.
//!
//! Walks the polyfill's [`ExecutorIr`], building one
//! [`wasm_runtime_layer::Instance`] per `InstantiateModule`
//! directive, then resolves the component's exports to runtime-layer
//! function handles. The driver is target-agnostic: native and web
//! differ only in how the IR is produced (see
//! [`super::translate`]).

use wasm_runtime_layer::{Extern as RuntimeExtern, Imports, Instance as RuntimeInstance};

use crate::component::{Component, ExternType, ExternalName, FunctionType};
use crate::engine::Engine;
use crate::error::{Error, InstantiationError, Result};
use crate::instance::{ExportedFunction, Instance};
use crate::store::Store;

use super::ir::{
    CoreInstanceExport, CoreSourceItem, ExecutorIr, ExportSpec, ImportSource, Initializer,
    ModuleEntry,
};

/// Translate the component's bytes into the executor's IR and drive
/// instantiation against `store`.
pub fn instantiate<T: 'static>(
    engine: &Engine,
    component: &Component,
    store: &mut Store<T>,
) -> Result<Instance> {
    let ir = super::translate(engine, &component.bytes)?;

    let mut core_instances: Vec<RuntimeInstance> = Vec::new();
    for initializer in ir.initializers.iter() {
        match initializer {
            Initializer::InstantiateModule {
                module_index,
                imports,
            } => {
                let entry = ir.modules.get(*module_index).ok_or_else(|| {
                    internal("module index in IR initializer is out of bounds")
                })?;
                let runtime_imports = build_imports(&ir, &core_instances, store, entry, imports)?;
                let instance =
                    RuntimeInstance::new(store.inner_mut(), &entry.runtime, &runtime_imports)
                        .map_err(InstantiationError::SubstrateFailure)
                        .map_err(Error::Instantiation)?;
                core_instances.push(instance);
            }
        }
    }

    let function_exports = collect_function_exports(component, &ir, &core_instances, store)?;
    Ok(Instance {
        core_instances: core_instances.into_boxed_slice(),
        function_exports,
    })
}

/// Build the [`Imports`] table for a single module instantiation by
/// pairing each declared module import with the [`ImportSource`] the
/// IR supplies for it.
fn build_imports<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    store: &mut Store<T>,
    entry: &ModuleEntry,
    sources: &[ImportSource],
) -> Result<Imports> {
    if entry.imports.len() != sources.len() {
        return Err(internal(
            "module's declared import count does not match the IR's per-module import sources",
        ));
    }
    let mut imports = Imports::default();
    for (module_import, source) in entry.imports.iter().zip(sources.iter()) {
        let value = resolve_source(ir, core_instances, store, source)?;
        imports.define(&module_import.host, &module_import.name, value);
    }
    Ok(imports)
}

/// Resolve an [`ImportSource`] to the runtime-layer [`Extern`] the
/// import's slot accepts.
fn resolve_source<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    store: &mut Store<T>,
    source: &ImportSource,
) -> Result<RuntimeExtern> {
    match source {
        ImportSource::CoreInstanceExport(export) => {
            resolve_core_instance_export(ir, core_instances, store, export)
        }
    }
}

fn resolve_core_instance_export<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    store: &mut Store<T>,
    export: &CoreInstanceExport,
) -> Result<RuntimeExtern> {
    let runtime_instance = core_instances.get(export.instance_index).ok_or_else(|| {
        internal("CoreInstanceExport.instance_index is out of bounds")
    })?;
    // The runtime-layer `Instance::get_export` is name-keyed. For
    // index-style references we walk the owning module's inverted
    // export table to recover the declared name. The owning module
    // for a runtime instance is whichever module `module_index` the
    // IR's `Initializer::InstantiateModule` directive named for
    // that instance — we can recover it from the same position in
    // `ir.initializers`.
    let module_index = match ir.initializers.get(export.instance_index) {
        Some(Initializer::InstantiateModule { module_index, .. }) => *module_index,
        None => {
            return Err(internal(
                "CoreInstanceExport refers to an instance with no matching initializer",
            ));
        }
    };
    let owning_module = ir.modules.get(module_index).ok_or_else(|| {
        internal("module index in initializer is out of bounds for ir.modules")
    })?;
    let name: &str = match &export.item {
        CoreSourceItem::Name(s) => s.as_str(),
        CoreSourceItem::Index(entity) => owning_module
            .entity_to_name
            .get(entity)
            .map(String::as_str)
            .ok_or_else(|| {
                internal("CoreSourceItem::Index has no corresponding export name")
            })?,
    };
    runtime_instance
        .get_export(store.inner(), name)
        .ok_or_else(|| internal("module did not export the named item at runtime"))
}

/// Walk the IR's exports, build a [`wasm_runtime_layer::Func`] per
/// lifted-function export, and pair it with the polyfill
/// [`FunctionType`] declared in [`Component::exports`] so the
/// polyfill's [`Func::call`] knows how to lower its arguments and
/// lift its result.
///
/// [`Func::call`]: crate::Func::call
fn collect_function_exports<T: 'static>(
    component: &Component,
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    store: &mut Store<T>,
) -> Result<Box<[ExportedFunction]>> {
    let mut out = Vec::with_capacity(ir.exports.len());
    for ExportSpec { name, source } in ir.exports.iter() {
        let extern_value = resolve_source(ir, core_instances, store, source)?;
        let RuntimeExtern::Func(func) = extern_value else {
            return Err(internal(
                "lifted-function export resolved to a non-function core item",
            ));
        };
        let signature = lookup_function_signature(component, name)?;
        out.push(ExportedFunction {
            name: name.clone(),
            func,
            signature,
        });
    }
    Ok(out.into_boxed_slice())
}

/// Find the [`FunctionType`] the polyfill's parsed-component value
/// declared for the export named `name`.
fn lookup_function_signature(component: &Component, name: &str) -> Result<FunctionType> {
    for export in component.exports.iter() {
        let matches = match &export.name {
            ExternalName::Plain(text) => text == name,
            ExternalName::Interface(id) => id.to_string() == name,
        };
        if matches {
            return match &export.ty {
                ExternType::Function(ty) => Ok(ty.clone()),
                _ => Err(internal(
                    "lifted-function export's polyfill type is not a function",
                )),
            };
        }
    }
    Err(internal(
        "lifted-function export not present in the polyfill's parsed-component view",
    ))
}

fn internal(message: &str) -> Error {
    Error::Internal {
        message: message.to_owned(),
    }
}
