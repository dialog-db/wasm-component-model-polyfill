//! The cross-target executor driver.
//!
//! Walks the polyfill's [`ExecutorIr`], building one
//! [`wasm_runtime_layer::Instance`] per `InstantiateModule`
//! directive, populating the canonical-ABI runtime state slabs as
//! `Extract*` directives are encountered, and constructing host
//! trampolines for `LowerImport` directives. The driver is target-
//! agnostic: native and web differ only in how the IR is produced
//! (see [`super::translate`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;

use wasm_runtime_layer::{
    Extern as RuntimeExtern, Func as RuntimeFunc, Global as RuntimeGlobal, Imports,
    Instance as RuntimeInstance, Val as RuntimeVal, ValType as CoreType,
};

use crate::component::{Component, ExternType, ExternalName};
use crate::concurrency::InstanceId;
use crate::error::{Error, InstantiationError, LinkError, Result};
use crate::instance::{ExportedFunction, ExportedModule, Instance};
use crate::linker::{HostFuncBody, ImportBinding, InstanceRegistration, Linker, Resolution};
use crate::module::Module;
use crate::resource::{ResourceTableRuntime, TableId};
use crate::store::StoreContext;

use super::ResourceDestructor;
use super::intrinsics::{
    build_context_get, build_context_set, build_enter_sync_call, build_exit_sync_call,
    build_resource_transfer, build_transcoder, build_trap,
};
use crate::abi::runtime_state::AbiRuntimeState;

use super::ir::{
    CoreInstanceExport, CoreSourceItem, ExecutorIr, ExportSpec, ImportSource, Initializer,
    LoweringSpec, ModuleEntry, ModuleSource, ResourceSpec, TrampolineSpec,
};
use super::trampoline::{
    ResourceRuntime, build_resource_drop_trampoline, build_resource_new_trampoline,
    build_resource_rep_trampoline, build_trampoline,
};

/// The runtime items the executor has produced so far while walking
/// the plan: core instances in instantiation order, every trampoline
/// (built upfront), and one `may_leave` flags global per component
/// instance.
struct RuntimeItems {
    core_instances: Vec<RuntimeInstance>,
    trampolines: Vec<RuntimeFunc>,
    flags: Vec<RuntimeGlobal>,
    task_may_block: RuntimeGlobal,
}

/// Drive instantiation of the component's plan against `store`.
///
/// Takes the linker so host-function trampolines can dispatch to the
/// registered [`HostFunc`](crate::linker::HostFunc) payloads at call
/// time, and the linker's resolution of the component's imports so
/// every lowered import and imported resource reaches the
/// registration the resolver chose. The linker is borrowed only for
/// the duration of instantiation; the trampolines hold `Arc` clones
/// of the closures they need.
pub fn instantiate<T: 'static>(
    component: &Component,
    store: &mut StoreContext<'_, T>,
    linker: &Linker<T>,
    resolution: &Resolution,
) -> Result<Instance> {
    let ir: &ExecutorIr = &component.ir;

    // Resolve each `ResourceSpec` against the linker's registered
    // host resources before building the runtime state. The host
    // destructor closure is captured by every resource trampoline
    // the executor builds for that resource.
    let mut resource_runtimes: Vec<ResourceRuntime<T>> = Vec::with_capacity(ir.resources.len());
    for spec in ir.resources.iter() {
        resource_runtimes.push(resolve_resource_runtime(
            linker, component, resolution, spec,
        )?);
    }

    // The store learns every destructor this instantiation introduces,
    // so a handle the host holds can be released through the store.
    for runtime in &resource_runtimes {
        store.register_destructor(runtime.type_id, runtime.destructor.clone());
    }

    // One fresh handle table per component instance, shared by every
    // resource type the instance uses: the canonical ABI keeps handles
    // per instance, and an index of one type can name an entry of
    // another, which the typed lookups reject.
    let mut instance_tables: HashMap<usize, TableId> = HashMap::new();
    let resource_tables: Vec<Option<ResourceTableRuntime>> = ir
        .resource_tables
        .iter()
        .map(|spec| {
            spec.as_ref().and_then(|spec| {
                resource_runtimes
                    .get(spec.resource_index)
                    .map(|runtime| ResourceTableRuntime {
                        table: *instance_tables
                            .entry(spec.instance)
                            .or_insert_with(TableId::fresh),
                        type_id: runtime.type_id,
                        resource_index: spec.resource_index,
                        defining: spec.defining,
                        guest_defined: matches!(runtime.destructor, ResourceDestructor::Local(_)),
                    })
            })
        })
        .collect();

    // One instance record per component instance of this
    // instantiation: the entry gate, the backpressure counter, the
    // exclusive thread, and the two flags a call consults. The
    // adapters name their instances by the translator's index, which
    // this list maps onto the store-wide identity.
    let component_instances: Vec<InstanceId> = {
        let mut guard = store
            .tables()
            .lock()
            .map_err(|_| internal("resource handle tables lock poisoned"))?;
        (0..ir.num_component_instances)
            .map(|_| guard.tasks.insert_instance())
            .collect()
    };

    let abi_state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
        ir.num_runtime_memories,
        ir.num_runtime_reallocs,
        ir.num_runtime_post_returns,
        resource_tables,
        component_instances,
    )));

    // Build every trampoline upfront. Trampolines never depend on
    // core-instance state at construction (memories, reallocs, etc.
    // are read out of the shared `AbiRuntimeState` at call time), so
    // the resulting runtime-layer `Func`s can be slotted into the
    // import table for any module that references them via
    // `CoreDef::Trampoline`.
    let mut trampolines: Vec<RuntimeFunc> = Vec::with_capacity(ir.trampoline_specs.len());
    for spec in ir.trampoline_specs.iter() {
        let func = build_runtime_trampoline(
            spec,
            component,
            linker,
            resolution,
            store,
            &abi_state,
            &resource_runtimes,
        )?;
        trampolines.push(func);
    }

    // One `may_leave` flags global per component instance. Adapter
    // modules import it; a fresh instantiation starts with the flag
    // set, because every instance may be left until an adapter is
    // in the middle of translating values across its boundary.
    let flags: Vec<RuntimeGlobal> = (0..ir.num_component_instances)
        .map(|_| RuntimeGlobal::new(store.runtime_mut(), RuntimeVal::I32(1), true))
        .collect();

    let task_may_block = RuntimeGlobal::new(store.runtime_mut(), RuntimeVal::I32(1), true);
    let mut items = RuntimeItems {
        core_instances: Vec::new(),
        trampolines,
        flags,
        task_may_block,
    };

    for initializer in ir.initializers.iter() {
        match initializer {
            Initializer::InstantiateModule {
                module_index,
                imports,
            } => {
                let entry = ir
                    .modules
                    .get(*module_index)
                    .ok_or_else(|| internal("module index in IR initializer is out of bounds"))?;
                let runtime_imports = build_imports(ir, &items, store, entry, imports)?;
                let instance = RuntimeInstance::new(
                    store.runtime_mut(),
                    &entry.module.inner,
                    &runtime_imports,
                )
                .map_err(InstantiationError::SubstrateFailure)
                .map_err(Error::from)?;
                items.core_instances.push(instance);
            }
            Initializer::InstantiateImportedModule { source, imports } => {
                let module = lookup_module(linker, component, resolution, source)?;
                let mut runtime_imports = Imports::default();
                for import in imports.iter() {
                    let value = resolve_source(ir, &items, store, &import.source)?;
                    runtime_imports.define(&import.module, &import.name, value);
                }
                let instance =
                    RuntimeInstance::new(store.runtime_mut(), &module.inner, &runtime_imports)
                        .map_err(InstantiationError::SubstrateFailure)
                        .map_err(Error::from)?;
                items.core_instances.push(instance);
            }
            Initializer::ExtractMemory { slot, source } => {
                let extern_value = resolve_source(ir, &items, store, source)?;
                let RuntimeExtern::Memory(memory) = extern_value else {
                    return Err(internal(
                        "ExtractMemory directive resolved to a non-memory item",
                    ));
                };
                let mut state = abi_state
                    .lock()
                    .map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.memories.get_mut(*slot) {
                    *s = Some(memory);
                } else {
                    return Err(internal("ExtractMemory slot out of bounds"));
                }
            }
            Initializer::DefineResource { resource_index } => {
                let (runtime, spec) = resource_runtimes
                    .get(*resource_index)
                    .zip(ir.resources.get(*resource_index))
                    .ok_or_else(|| internal("DefineResource resource_index out of bounds"))?;
                let (ResourceSpec::Local { destructor, .. }, ResourceDestructor::Local(slot)) =
                    (spec, &runtime.destructor)
                else {
                    return Err(internal(
                        "DefineResource directive names a resource that is not locally defined",
                    ));
                };
                if let Some(source) = destructor {
                    let extern_value = resolve_source(ir, &items, store, source)?;
                    let RuntimeExtern::Func(function) = extern_value else {
                        return Err(internal(
                            "DefineResource directive resolved to a non-function item",
                        ));
                    };
                    let ty = function.ty(store.runtime());
                    if ty.params() != [CoreType::I32] || !ty.results().is_empty() {
                        return Err(Error::from(InstantiationError::SubstrateFailure(anyhow!(
                            "the destructor of a locally-defined resource must have the core type \
                             (func (param i32)), found {ty:?}"
                        ))));
                    }
                    *slot
                        .lock()
                        .map_err(|_| internal("resource destructor slot poisoned"))? =
                        Some(function);
                }
            }
            Initializer::ExtractRealloc { slot, source } => {
                let extern_value = resolve_source(ir, &items, store, source)?;
                let RuntimeExtern::Func(realloc) = extern_value else {
                    return Err(internal(
                        "ExtractRealloc directive resolved to a non-function item",
                    ));
                };
                let mut state = abi_state
                    .lock()
                    .map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.reallocs.get_mut(*slot) {
                    *s = Some(realloc);
                } else {
                    return Err(internal("ExtractRealloc slot out of bounds"));
                }
            }
            Initializer::ExtractPostReturn { slot, source } => {
                let extern_value = resolve_source(ir, &items, store, source)?;
                let RuntimeExtern::Func(post_return) = extern_value else {
                    return Err(internal(
                        "ExtractPostReturn directive resolved to a non-function item",
                    ));
                };
                let mut state = abi_state
                    .lock()
                    .map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.post_returns.get_mut(*slot) {
                    *s = Some(post_return);
                } else {
                    return Err(internal("ExtractPostReturn slot out of bounds"));
                }
            }
        }
    }

    let function_exports = collect_function_exports(ir, &items, store)?;
    let module_exports = collect_module_exports(ir, linker, component, resolution)?;
    Ok(Instance {
        core_instances: items.core_instances.into_boxed_slice(),
        function_exports,
        instance_exports: ir.instance_exports.clone(),
        module_exports,
        abi_state,
        store_id: store.id(),
    })
}

/// Build the runtime-layer trampoline for one entry in
/// [`ExecutorIr::trampoline_specs`]. Dispatches by variant: lowered
/// imports become host-function trampolines, resource intrinsics
/// become per-resource handle-table operations.
#[allow(clippy::too_many_arguments)]
fn build_runtime_trampoline<T: 'static>(
    spec: &TrampolineSpec,
    component: &Component,
    linker: &Linker<T>,
    resolution: &Resolution,
    store: &mut StoreContext<'_, T>,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    resource_runtimes: &[ResourceRuntime<T>],
) -> Result<RuntimeFunc> {
    match spec {
        TrampolineSpec::LowerImport(lowering) => {
            let host_func = lookup_host_func(linker, component, resolution, lowering)?;
            Ok(build_trampoline(
                store,
                lowering,
                abi_state.clone(),
                host_func,
            ))
        }
        TrampolineSpec::ResourceDrop { table_index } => {
            let table = resource_table(abi_state, *table_index)?;
            let runtime = resource_runtimes
                .get(table.resource_index)
                .ok_or_else(|| internal("ResourceDrop.table_index names an unknown resource"))?;
            Ok(build_resource_drop_trampoline(
                store,
                table,
                runtime.clone(),
            ))
        }
        TrampolineSpec::ResourceNew { table_index } => {
            let table = resource_table(abi_state, *table_index)?;
            Ok(build_resource_new_trampoline(store, table))
        }
        TrampolineSpec::ResourceRep { table_index } => {
            let table = resource_table(abi_state, *table_index)?;
            Ok(build_resource_rep_trampoline(store, table))
        }
        TrampolineSpec::Transcoder {
            op,
            from_memory,
            to_memory,
            signature,
        } => Ok(build_transcoder(
            store,
            *op,
            *from_memory,
            *to_memory,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::ResourceTransferOwn { signature } => Ok(build_resource_transfer(
            store,
            signature,
            abi_state.clone(),
            true,
        )),
        TrampolineSpec::ResourceTransferBorrow { signature } => Ok(build_resource_transfer(
            store,
            signature,
            abi_state.clone(),
            false,
        )),
        TrampolineSpec::Trap { signature } => Ok(build_trap(store, signature)),
        TrampolineSpec::EnterSyncCall { signature } => {
            Ok(build_enter_sync_call(store, signature, abi_state.clone()))
        }
        TrampolineSpec::ExitSyncCall { signature } => Ok(build_exit_sync_call(store, signature)),
        TrampolineSpec::ContextGet { slot, signature } => {
            Ok(build_context_get(store, *slot, signature))
        }
        TrampolineSpec::ContextSet { slot, signature } => {
            Ok(build_context_set(store, *slot, signature))
        }
    }
}

/// The registration an import resolved to, and the item name to
/// look up in it, or the structured link error when the resolver
/// recorded no registration for it. An interface-named import
/// resolves to the chosen interface entry; a plain-named instance
/// import resolves to the nested root entry under its name; either
/// is then walked one nested registration per leading segment of
/// `path`, and the last segment is the item. A plain-named function,
/// resource, or module import resolves to the root entry under the
/// import's own name, with an empty `path`.
fn registration_and_item<'l, T: 'static>(
    linker: &'l Linker<T>,
    component: &Component,
    resolution: &Resolution,
    import_index: usize,
    path: &[String],
) -> Result<(&'l InstanceRegistration<T>, String)> {
    let import = component
        .imports
        .get(import_index)
        .ok_or_else(|| internal("import index in executor spec out of bounds"))?;
    let unresolved = || {
        Error::from(LinkError::UnresolvedImport {
            import: import.name.clone(),
        })
    };
    let mut registration = match resolution.bindings.get(import_index) {
        Some(ImportBinding::Resolved { chosen }) => {
            linker.registration_for(chosen).ok_or_else(unresolved)?
        }
        Some(ImportBinding::Root) => {
            let ExternalName::Plain(name) = &import.name else {
                return Err(internal("a root binding on an interface-named import"));
            };
            let root = linker.root_registration();
            match &import.ty {
                ExternType::Instance(_) => root.instance(name).ok_or_else(unresolved)?,
                _ => return Ok((root, name.clone())),
            }
        }
        Some(ImportBinding::Vacuous) | None => return Err(unresolved()),
    };
    let (item, parents) = path.split_last().ok_or_else(unresolved)?;
    for segment in parents {
        registration = registration.instance(segment).ok_or_else(unresolved)?;
    }
    Ok((registration, item.clone()))
}

/// Look up a host-resource registration that satisfies the given
/// [`ResourceSpec`]. Mirrors [`lookup_host_func`] but returns the
/// destructor and identity for the resource.
fn resolve_resource_runtime<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    resolution: &Resolution,
    spec: &ResourceSpec,
) -> Result<ResourceRuntime<T>> {
    let (import_index, path) = match spec {
        ResourceSpec::Local { .. } => return Ok(ResourceRuntime::local()),
        ResourceSpec::Imported { import_index, path } => (*import_index, path),
    };
    let (registration, label) =
        registration_and_item(linker, component, resolution, import_index, path)?;
    let host = registration.resource(&label).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: component.imports[import_index].name.clone(),
        })
    })?;
    Ok(ResourceRuntime::from_registration(host))
}

/// The module a [`ModuleSource`] names: a compiled module of the
/// component, or the module the linker registered for the import.
fn lookup_module<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    resolution: &Resolution,
    source: &ModuleSource,
) -> Result<Module> {
    match source {
        ModuleSource::Static(index) => component
            .ir
            .modules
            .get(*index)
            .map(|entry| entry.module.clone())
            .ok_or_else(|| internal("module source names a module slot outside the IR")),
        ModuleSource::Import { import_index, path } => {
            let (registration, item) =
                registration_and_item(linker, component, resolution, *import_index, path)?;
            registration.module(&item).cloned().ok_or_else(|| {
                Error::from(LinkError::UnresolvedImport {
                    import: component.imports[*import_index].name.clone(),
                })
            })
        }
    }
}

/// Look up the host-function payload registered against the import
/// the lowering targets. Returns the closure as an `Arc` for the
/// trampoline to capture.
fn lookup_host_func<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    resolution: &Resolution,
    spec: &LoweringSpec,
) -> Result<Arc<HostFuncBody<T>>> {
    let (registration, item_name) =
        registration_and_item(linker, component, resolution, spec.import_index, &spec.path)?;
    let host = registration.func(&item_name).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: component.imports[spec.import_index].name.clone(),
        })
    })?;
    Ok(host.call.clone())
}

/// Build the [`Imports`] table for a single module instantiation by
/// pairing each declared module import with the [`ImportSource`] the
/// IR supplies for it.
fn build_imports<T: 'static>(
    ir: &ExecutorIr,
    items: &RuntimeItems,
    store: &mut StoreContext<'_, T>,
    entry: &ModuleEntry,
    sources: &[ImportSource],
) -> Result<Imports> {
    let declared = entry.module.imports();
    if declared.len() != sources.len() {
        return Err(internal(
            "module's declared import count does not match the IR's per-module import sources",
        ));
    }
    let mut imports = Imports::default();
    for (module_import, source) in declared.iter().zip(sources.iter()) {
        let value = resolve_source(ir, items, store, source)?;
        imports.define(&module_import.module, &module_import.name, value);
    }
    Ok(imports)
}

/// Resolve an [`ImportSource`] to the runtime-layer [`Extern`] the
/// import's slot accepts.
fn resolve_source<T: 'static>(
    ir: &ExecutorIr,
    items: &RuntimeItems,
    store: &mut StoreContext<'_, T>,
    source: &ImportSource,
) -> Result<RuntimeExtern> {
    match source {
        ImportSource::CoreInstanceExport(export) => {
            resolve_core_instance_export(ir, &items.core_instances, store, export)
        }
        ImportSource::Trampoline(idx) => items
            .trampolines
            .get(*idx)
            .cloned()
            .map(RuntimeExtern::Func)
            .ok_or_else(|| internal("ImportSource::Trampoline index is out of bounds")),
        ImportSource::InstanceFlags(idx) => items
            .flags
            .get(*idx)
            .cloned()
            .map(RuntimeExtern::Global)
            .ok_or_else(|| internal("ImportSource::InstanceFlags index is out of bounds")),
        ImportSource::TaskMayBlock => Ok(RuntimeExtern::Global(items.task_may_block.clone())),
    }
}

fn resolve_core_instance_export<T: 'static>(
    ir: &ExecutorIr,
    core_instances: &[RuntimeInstance],
    store: &mut StoreContext<'_, T>,
    export: &CoreInstanceExport,
) -> Result<RuntimeExtern> {
    let runtime_instance = core_instances
        .get(export.instance_index)
        .ok_or_else(|| internal("CoreInstanceExport.instance_index is out of bounds"))?;
    let module_index = *ir
        .runtime_instance_to_module
        .get(export.instance_index)
        .ok_or_else(|| internal("instance_index out of bounds for runtime_instance_to_module"))?;
    let name: &str = match &export.item {
        CoreSourceItem::Name(s) => s.as_str(),
        CoreSourceItem::Index(entity) => {
            let owning_module = module_index
                .and_then(|index| ir.modules.get(index))
                .ok_or_else(|| {
                    internal("an indexed core export names an instance of an imported module")
                })?;
            owning_module
                .entity_to_name
                .get(entity)
                .map(String::as_str)
                .ok_or_else(|| internal("CoreSourceItem::Index has no corresponding export name"))?
        }
    };
    runtime_instance
        .get_export(store.runtime(), name)
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
    items: &RuntimeItems,
    store: &mut StoreContext<'_, T>,
) -> Result<Box<[ExportedFunction]>> {
    let mut out = Vec::with_capacity(ir.exports.len());
    for ExportSpec {
        name,
        path,
        source,
        signature,
        options,
    } in ir.exports.iter()
    {
        let extern_value = resolve_source(ir, items, store, source)?;
        let RuntimeExtern::Func(func) = extern_value else {
            return Err(internal(
                "lifted-function export resolved to a non-function core item",
            ));
        };
        out.push(ExportedFunction {
            name: name.clone(),
            path: path.clone(),
            func,
            signature: signature.clone(),
            options: options.clone(),
        });
    }
    Ok(out.into_boxed_slice())
}

/// Walk the IR's module exports and pair each with the module it
/// names: one the translator compiled, or one the linker registered
/// for an import the component re-exports.
fn collect_module_exports<T: 'static>(
    ir: &ExecutorIr,
    linker: &Linker<T>,
    component: &Component,
    resolution: &Resolution,
) -> Result<Box<[ExportedModule]>> {
    let mut out = Vec::with_capacity(ir.module_exports.len());
    for spec in ir.module_exports.iter() {
        out.push(ExportedModule {
            name: spec.name.clone(),
            path: spec.path.clone(),
            module: lookup_module(linker, component, resolution, &spec.source)?,
        });
    }
    Ok(out.into_boxed_slice())
}

fn internal(message: &str) -> Error {
    Error::internal(message)
}

/// The runtime data of one resource table of the instantiation.
fn resource_table(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    table_index: usize,
) -> Result<ResourceTableRuntime> {
    let state = abi_state
        .lock()
        .map_err(|_| internal("ABI state poisoned"))?;
    state
        .resource_tables
        .get(table_index)
        .copied()
        .flatten()
        .ok_or_else(|| {
            internal("resource trampoline names a table the instantiation does not hold")
        })
}
