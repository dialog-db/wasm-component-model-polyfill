//! The cross-target executor driver.
//!
//! Walks the polyfill's [`ExecutorIr`], building one
//! [`wasm_runtime_layer::Instance`] per `InstantiateModule`
//! directive, populating the canonical-ABI runtime state slabs as
//! `Extract*` directives are encountered, and constructing host
//! trampolines for `LowerImport` directives. The driver is target-
//! agnostic: native and web differ only in how the IR is produced
//! (see [`super::translate`]).

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{
    Extern as RuntimeExtern, Func as RuntimeFunc, Imports, Instance as RuntimeInstance,
};

use crate::component::{Component, ExternalName};
use crate::engine::Engine;
use crate::error::{Error, InstantiationError, LinkError, Result};
use crate::instance::{ExportedFunction, Instance};
use crate::linker::{HostFuncBody, Linker};
use crate::store::Store;

use super::ir::{
    CoreInstanceExport, CoreSourceItem, ExecutorIr, ExportSpec, ImportSource, Initializer,
    LoweringSpec, ModuleEntry, ResourceSpec, TrampolineSpec,
};
use super::trampoline::{
    build_resource_drop_trampoline, build_resource_new_trampoline, build_resource_rep_trampoline,
    build_trampoline, AbiRuntimeState, ResourceRuntime,
};

/// Translate the component's bytes into the executor's IR and drive
/// instantiation against `store`.
///
/// Takes the linker so host-function trampolines can dispatch to the
/// registered [`HostFunc`](crate::linker::HostFunc) payloads at call
/// time. The linker is borrowed only for the duration of
/// instantiation; the trampolines hold `Arc` clones of the closures
/// they need.
pub fn instantiate<T: 'static>(
    engine: &Engine,
    component: &Component,
    store: &mut Store<T>,
    linker: &Linker<T>,
) -> Result<Instance> {
    let ir = super::translate(engine, component)?;

    // Resolve each `ResourceSpec` against the linker's registered
    // host resources before building the runtime state. The host
    // destructor closure is captured by every resource trampoline
    // the executor builds for that resource.
    let mut resource_runtimes: Vec<ResourceRuntime<T>> = Vec::with_capacity(ir.resources.len());
    for spec in ir.resources.iter() {
        resource_runtimes.push(resolve_resource_runtime(linker, component, spec)?);
    }

    let abi_state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
        ir.num_runtime_memories,
        ir.num_runtime_reallocs,
        ir.num_runtime_post_returns,
    )));

    // Build every trampoline upfront. Trampolines never depend on
    // core-instance state at construction (memories, reallocs, etc.
    // are read out of the shared `AbiRuntimeState` at call time), so
    // the resulting runtime-layer `Func`s can be slotted into the
    // import table for any module that references them via
    // `CoreDef::Trampoline`.
    let mut trampolines: Vec<Option<RuntimeFunc>> =
        Vec::with_capacity(ir.trampoline_specs.len());
    for spec in ir.trampoline_specs.iter() {
        let func = build_runtime_trampoline(
            spec,
            component,
            linker,
            store,
            &abi_state,
            &resource_runtimes,
        )?;
        trampolines.push(Some(func));
    }

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
                let runtime_imports =
                    build_imports(&ir, &core_instances, &trampolines, store, entry, imports)?;
                let instance =
                    RuntimeInstance::new(store.inner_mut(), &entry.runtime, &runtime_imports)
                        .map_err(InstantiationError::SubstrateFailure)
                        .map_err(Error::Instantiation)?;
                core_instances.push(instance);
            }
            Initializer::ExtractMemory { slot, source } => {
                let extern_value =
                    resolve_source(&ir, &core_instances, &trampolines, store, source)?;
                let RuntimeExtern::Memory(memory) = extern_value else {
                    return Err(internal(
                        "ExtractMemory directive resolved to a non-memory item",
                    ));
                };
                let mut state = abi_state.lock().map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.memories.get_mut(*slot) {
                    *s = Some(memory);
                } else {
                    return Err(internal("ExtractMemory slot out of bounds"));
                }
            }
            Initializer::ExtractRealloc { slot, source } => {
                let extern_value =
                    resolve_source(&ir, &core_instances, &trampolines, store, source)?;
                let RuntimeExtern::Func(realloc) = extern_value else {
                    return Err(internal(
                        "ExtractRealloc directive resolved to a non-function item",
                    ));
                };
                let mut state = abi_state.lock().map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.reallocs.get_mut(*slot) {
                    *s = Some(realloc);
                } else {
                    return Err(internal("ExtractRealloc slot out of bounds"));
                }
            }
            Initializer::ExtractPostReturn { slot, source } => {
                let extern_value =
                    resolve_source(&ir, &core_instances, &trampolines, store, source)?;
                let RuntimeExtern::Func(post_return) = extern_value else {
                    return Err(internal(
                        "ExtractPostReturn directive resolved to a non-function item",
                    ));
                };
                let mut state = abi_state.lock().map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.post_returns.get_mut(*slot) {
                    *s = Some(post_return);
                } else {
                    return Err(internal("ExtractPostReturn slot out of bounds"));
                }
            }
        }
    }

    let function_exports = collect_function_exports(&ir, &core_instances, &trampolines, store)?;
    Ok(Instance {
        core_instances: core_instances.into_boxed_slice(),
        function_exports,
        abi_state,
    })
}

/// Build the runtime-layer trampoline for one entry in
/// [`ExecutorIr::trampoline_specs`]. Dispatches by variant: lowered
/// imports become host-function trampolines, resource intrinsics
/// become per-resource handle-table operations.
fn build_runtime_trampoline<T: 'static>(
    spec: &TrampolineSpec,
    component: &Component,
    linker: &Linker<T>,
    store: &mut Store<T>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    resource_runtimes: &[ResourceRuntime<T>],
) -> Result<RuntimeFunc> {
    match spec {
        TrampolineSpec::LowerImport(lowering) => {
            let host_func = lookup_host_func(linker, component, lowering)?;
            Ok(build_trampoline(
                store,
                lowering,
                abi_state.clone(),
                host_func,
            ))
        }
        TrampolineSpec::ResourceDrop { resource_index } => {
            let runtime = resource_runtimes
                .get(*resource_index)
                .ok_or_else(|| internal("ResourceDrop.resource_index out of bounds"))?;
            Ok(build_resource_drop_trampoline(store, runtime.clone()))
        }
        TrampolineSpec::ResourceNew { resource_index } => {
            let runtime = resource_runtimes
                .get(*resource_index)
                .ok_or_else(|| internal("ResourceNew.resource_index out of bounds"))?;
            Ok(build_resource_new_trampoline(store, runtime.clone()))
        }
        TrampolineSpec::ResourceRep { resource_index } => {
            let runtime = resource_runtimes
                .get(*resource_index)
                .ok_or_else(|| internal("ResourceRep.resource_index out of bounds"))?;
            Ok(build_resource_rep_trampoline(store, runtime.clone()))
        }
    }
}

/// Look up a host-resource registration that satisfies the given
/// [`ResourceSpec`]. Mirrors [`lookup_host_func`] but returns the
/// destructor and identity for the resource.
fn resolve_resource_runtime<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    spec: &ResourceSpec,
) -> Result<ResourceRuntime<T>> {
    let import = component
        .imports
        .get(spec.import_index)
        .ok_or_else(|| internal("ResourceSpec.import_index out of bounds"))?;
    let chosen = match &import.name {
        ExternalName::Interface(id) => id,
        ExternalName::Plain(_) => {
            return Err(Error::Link(LinkError::UnsupportedRegistration {
                import: import.name.clone(),
                reason: "plain-named imports require host-item registration",
            }));
        }
    };
    let registration = linker.registration_for(chosen).ok_or_else(|| {
        Error::Link(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    })?;
    let label = match &spec.item_name {
        Some(name) => name.as_str(),
        None => {
            return Err(internal(
                "top-level resource imports without an inner-instance path are not yet supported",
            ));
        }
    };
    let host = registration.resource(label).ok_or_else(|| {
        Error::Link(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    })?;
    Ok(ResourceRuntime::from_registration(host))
}

/// Look up the host-function payload registered against the import
/// the lowering targets. Returns the closure as an `Arc` for the
/// trampoline to capture.
fn lookup_host_func<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    spec: &LoweringSpec,
) -> Result<Arc<HostFuncBody<T>>> {
    let import = component
        .imports
        .get(spec.import_index)
        .ok_or_else(|| internal("LoweringSpec.import_index out of bounds"))?;
    let chosen = match &import.name {
        ExternalName::Interface(id) => id,
        ExternalName::Plain(_) => {
            return Err(Error::Link(LinkError::UnsupportedRegistration {
                import: import.name.clone(),
                reason: "plain-named imports require host-item registration",
            }));
        }
    };
    let registration = linker.registration_for(chosen).ok_or_else(|| {
        Error::Link(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    })?;
    let item_name = match &spec.item_name {
        Some(name) => name.as_str(),
        None => {
            // The import IS the function (top-level function
            // import). The polyfill's resolver rejects plain-named
            // imports above, so reaching here means an interface-
            // typed import has no path leaf — which the
            // synchronous baseline tests do not produce.
            return Err(internal(
                "interface-typed lowered import had no item-path leaf",
            ));
        }
    };
    let host = registration.func(item_name).ok_or_else(|| {
        Error::Link(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    })?;
    Ok(host.call.clone())
}

/// Build the [`Imports`] table for a single module instantiation by
/// pairing each declared module import with the [`ImportSource`] the
/// IR supplies for it.
fn build_imports<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    trampolines: &[Option<RuntimeFunc>],
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
        let value = resolve_source(ir, core_instances, trampolines, store, source)?;
        imports.define(&module_import.host, &module_import.name, value);
    }
    Ok(imports)
}

/// Resolve an [`ImportSource`] to the runtime-layer [`Extern`] the
/// import's slot accepts.
fn resolve_source<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    trampolines: &[Option<RuntimeFunc>],
    store: &mut Store<T>,
    source: &ImportSource,
) -> Result<RuntimeExtern> {
    match source {
        ImportSource::CoreInstanceExport(export) => {
            resolve_core_instance_export(ir, core_instances, store, export)
        }
        ImportSource::Trampoline(idx) => {
            let func = trampolines
                .get(*idx)
                .and_then(|slot| slot.clone())
                .ok_or_else(|| {
                    internal(
                        "ImportSource::Trampoline references a lowering not yet constructed",
                    )
                })?;
            Ok(RuntimeExtern::Func(func))
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
    let module_index = *ir
        .runtime_instance_to_module
        .get(export.instance_index)
        .ok_or_else(|| internal("instance_index out of bounds for runtime_instance_to_module"))?;
    let owning_module = ir.modules.get(module_index).ok_or_else(|| {
        internal("module index in runtime_instance_to_module is out of bounds for ir.modules")
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
/// [`FunctionType`] the IR projected onto the [`ExportSpec`] so the
/// polyfill's [`Func::call`] knows how to lower its arguments and
/// lift its result.
///
/// [`FunctionType`]: crate::FunctionType
/// [`Func::call`]: crate::Func::call
fn collect_function_exports<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    trampolines: &[Option<RuntimeFunc>],
    store: &mut Store<T>,
) -> Result<Box<[ExportedFunction]>> {
    let mut out = Vec::with_capacity(ir.exports.len());
    for ExportSpec {
        name,
        source,
        signature,
        options,
    } in ir.exports.iter()
    {
        let extern_value = resolve_source(ir, core_instances, trampolines, store, source)?;
        let RuntimeExtern::Func(func) = extern_value else {
            return Err(internal(
                "lifted-function export resolved to a non-function core item",
            ));
        };
        out.push(ExportedFunction {
            name: name.clone(),
            func,
            signature: signature.clone(),
            options: options.clone(),
        });
    }
    Ok(out.into_boxed_slice())
}

fn internal(message: &str) -> Error {
    Error::Internal {
        message: message.to_owned(),
    }
}
