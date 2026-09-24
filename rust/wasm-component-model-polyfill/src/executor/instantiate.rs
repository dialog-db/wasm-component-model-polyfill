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
    Extern as RuntimeExtern, Func as RuntimeFunc, Imports, Instance as RuntimeInstance,
    ValType as CoreType,
};

use crate::component::{Component, ExternType};
use crate::concurrency::InstanceId;
use crate::error::{Error, InstantiationError, LinkError, Result};
use crate::instance::{ExportedFunction, ExportedModule, Instance};
use crate::internal::{ComponentInternal, ErrorInternal, LinkerInternal};
use crate::internal::{InstanceParts, ModuleInternal};
use crate::linker::{HostFuncKind, ImportBinding, InstanceRegistration, Linker, Resolution};
use crate::module::Module;
use crate::resource::{HandleTables, ResourceTableRuntime, ResourceTypeId, TableId};
use crate::store::ResourceRecord;
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;
use crate::types::ResourceType;

use super::ResourceDestructor;
use super::build_async_start_call;
use super::build_prepare_call;
use super::build_subtask_cancel;
use super::build_sync_start_call;
use super::build_task_cancel;
use super::build_task_return;
use super::build_thread_yield;
use super::intrinsics::{
    build_backpressure_dec, build_backpressure_inc, build_context_get, build_context_set,
    build_end_transfer, build_enter_sync_call, build_exit_sync_call, build_resource_transfer,
    build_transcoder, build_trap,
};
use super::start_task::StartTask;
use super::waitable_builtins::{
    build_subtask_drop, build_waitable_join, build_waitable_set_drop, build_waitable_set_new,
    build_waitable_set_poll, build_waitable_set_wait,
};
use super::{build_copy, build_drop_end, build_future_new, build_stream_new};
use crate::abi::instance_flags::InstanceFlags;
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
/// (built upfront), and the may-leave flag of every component
/// instance.
struct RuntimeItems {
    core_instances: Vec<RuntimeInstance>,
    trampolines: Vec<RuntimeFunc>,
    flags: Vec<InstanceFlags>,
}

/// The store records one instantiation has added, and what the
/// store held before it.
///
/// An instantiation writes to the store before it can fail. The
/// instance records come first, because a resource the component
/// defines names the instance that defines it; the destructors and
/// the resource names follow, because the trampolines the plan
/// builds run against them. Both of the plan's own failures come
/// after that — a start function that traps, and an import the
/// linker cannot satisfy — and the records they leave would be one
/// more set in the store for every attempt.
///
/// So an instantiation reserves its records through this value and
/// takes them back when the plan fails. Wasmtime's store keeps what
/// a failed instantiation left in it, so this is hygiene rather than
/// parity: no instance of the store reaches a withdrawn record
/// either way, and an identity the store hands out again names a
/// record no instance holds.
struct ReservedRecords {
    /// The store's records, where the instance records live.
    tables: Arc<Mutex<HandleTables>>,
    /// How many instance records the store held before this
    /// instantiation reserved its own.
    instances_before: usize,
    /// The records this instantiation reserved, one per component
    /// instance of the plan, in the order the plan names them.
    instances: Vec<InstanceId>,
    /// What the store knew about each resource type this
    /// instantiation has registered, from before it registered
    /// anything for that type.
    resources: HashMap<ResourceTypeId, ResourceRecord>,
}

impl ReservedRecords {
    /// Reserve one instance record per component instance of the
    /// plan: the entry gate, the backpressure counter, the
    /// exclusive thread, and the suspend flag a call consults.
    ///
    /// The adapters name their instances by the translator's index,
    /// which the reserved list maps onto the store-wide identity.
    fn reserve<T: 'static>(store: &mut StoreContext<'_, T>, count: usize) -> Result<Self> {
        let tables = store.internal().tables_handle();
        let (instances_before, instances) = {
            let mut guard = tables
                .lock()
                .map_err(|_| internal("resource handle tables lock poisoned"))?;
            let before = guard.tasks.instances().len();
            let reserved = (0..count).map(|_| guard.tasks.insert_instance()).collect();
            (before, reserved)
        };
        Ok(Self {
            tables,
            instances_before,
            instances,
            resources: HashMap::new(),
        })
    }

    /// The instance records reserved, by the plan's own index for
    /// each component instance.
    fn instances(&self) -> &[InstanceId] {
        &self.instances
    }

    /// Register what the store is to know about a resource type
    /// this instantiation introduces: the destructor to run when a
    /// handle to it is released, and the name to render beside it.
    fn register_resource<T: 'static>(
        &mut self,
        store: &mut StoreContext<'_, T>,
        type_id: ResourceTypeId,
        name: Option<ResourceType>,
        destructor: ResourceDestructor<T>,
    ) {
        self.note(store, type_id);
        store
            .internal()
            .register_resource(type_id, name, destructor);
    }

    /// Record a label for the store to fall back on for `type_id`
    /// while no component in the store has named it.
    fn fallback_resource_name<T: 'static>(
        &mut self,
        store: &mut StoreContext<'_, T>,
        type_id: ResourceTypeId,
        name: ResourceType,
    ) {
        self.note(store, type_id);
        store.internal().fallback_resource_name(type_id, name);
    }

    /// Note what the store knows about `type_id` now, unless this
    /// instantiation has written to it already: a record taken
    /// after that would hold this instantiation's own registration
    /// rather than what preceded it.
    fn note<T: 'static>(&mut self, store: &mut StoreContext<'_, T>, type_id: ResourceTypeId) {
        self.resources
            .entry(type_id)
            .or_insert_with(|| store.internal().resource_record(type_id));
    }

    /// Take the records back, which is what a failed instantiation
    /// does: every resource type this instantiation registered goes
    /// back to what the store knew about it, and the instance
    /// records leave the store's list.
    ///
    /// A poisoned lock leaves the instance records where they are,
    /// because there is no list left to read or write. The
    /// registrations go back either way: they are the store's own
    /// data rather than the records behind the lock.
    fn withdraw<T: 'static>(self, store: &mut StoreContext<'_, T>) {
        for (_, record) in self.resources {
            store.internal().restore_resource(record);
        }
        let Ok(mut guard) = self.tables.lock() else {
            return;
        };
        guard.tasks.truncate_instances(self.instances_before);
    }
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
///
/// The store records the plan needs are reserved before it runs and
/// taken back when it fails, so a failed instantiation leaves the
/// store as it found it. See [`ReservedRecords`].
pub fn instantiate<T: 'static>(
    component: &Component,
    store: &mut StoreContext<'_, T>,
    linker: &Linker<T>,
    resolution: &Resolution,
) -> Result<Instance> {
    let mut reserved = ReservedRecords::reserve(store, component.ir().num_component_instances)?;
    match run_plan(component, store, linker, resolution, &mut reserved) {
        Ok(instance) => Ok(instance),
        Err(error) => {
            reserved.withdraw(store);
            Err(error)
        }
    }
}

/// Walk the plan against `store`, with the records `reserved` holds
/// standing in the store for the length of the walk.
///
/// Every failure of the walk returns here, and the caller is what
/// takes the reserved records back: a walk that returns an error has
/// left the store holding them.
fn run_plan<T: 'static>(
    component: &Component,
    store: &mut StoreContext<'_, T>,
    linker: &Linker<T>,
    resolution: &Resolution,
    reserved: &mut ReservedRecords,
) -> Result<Instance> {
    let ir: &ExecutorIr = component.ir();

    // One instance record per component instance of this
    // instantiation: the entry gate, the backpressure counter, the
    // exclusive thread, and the suspend flag a call consults. The
    // adapters name their instances by the translator's index, which
    // this list maps onto the store-wide identity. The records were
    // reserved before this walk began because a resource this
    // component defines names the instance that defines it, which is
    // the instance its destructor's task belongs to.
    let component_instances: Vec<InstanceId> = reserved.instances().to_vec();

    // Resolve each `ResourceSpec` against the linker's registered
    // host resources before building the runtime state. The host
    // destructor closure is captured by every resource trampoline
    // the executor builds for that resource.
    let mut resource_runtimes: Vec<ResourceRuntime<T>> = Vec::with_capacity(ir.resources.len());
    for spec in ir.resources.iter() {
        resource_runtimes.push(resolve_resource_runtime(
            linker,
            component,
            resolution,
            spec,
            &component_instances,
        )?);
    }

    // The component's own name for each resource it defines, by the
    // resource's index. This is the only source of a name for a
    // resource the component defines: nothing outside the binary
    // declares one. A resource has as many tables as there are
    // component instances that keep it, and a table names it as its
    // own instance's interface does, so the labels can differ from
    // table to table; the first table wins, which is the same
    // first-name-wins rule the store applies.
    let mut declared_names: HashMap<usize, ResourceType> = HashMap::new();
    for spec in ir.resource_tables.iter().flatten() {
        declared_names
            .entry(spec.resource_index)
            .or_insert_with(|| spec.resource_type.clone());
    }

    // The store learns every destructor this instantiation
    // introduces, so a handle the host holds can be released through
    // the store, and the name to render for the resource beside it.
    // A resource the component defines is named by the component's
    // own declaration; an imported one is named by the label the
    // host registered it under, which is also the label the
    // component imports it under, because that label is what the
    // resolver matched the registration by.
    for (index, runtime) in resource_runtimes.iter().enumerate() {
        let name = match &runtime.destructor {
            ResourceDestructor::Local { .. } => declared_names.get(&index).cloned(),
            ResourceDestructor::Host(_) => runtime.name.clone(),
        };
        reserved.register_resource(store, runtime.type_id, name, runtime.destructor.clone());
    }

    // The host resources the linker holds are then swept for their
    // labels, so that an error about a handle of one no component
    // brought in renders the label it was registered under rather
    // than nothing at all. The linker knows that label for every
    // registration, whether or not a component ever imports it, and
    // the store is the only place an error can read it from.
    //
    // Only the name travels: the destructor of a resource no
    // instance of this store holds is not the store's to run. And it
    // travels as a fallback, not as a name, because the sweep cannot
    // tell which of the linker's registrations a later
    // instantiation into this store will import: it visits every one
    // of them, including identities this component never mentions.
    // A fallback yields to the label a component's own import or
    // definition teaches, whether that component is instantiated
    // before this one or after it, so no store renders a label that
    // none of its components ever used while one of them did.
    //
    // A component that imports no resource skips the sweep. The sweep
    // visits and sorts every label the linker holds, which is work
    // proportional to the linker rather than to the component, and a
    // host that instantiates such a component many times would pay it
    // on every instantiation for nothing the component uses. A store
    // learns the fallbacks from the first instantiation of a component
    // that does import a resource. Until then it renders no label for
    // a handle the host minted of a linker's resource, exactly as a
    // store no component has been instantiated into does not.
    let imports_resource = ir
        .resources
        .iter()
        .any(|spec| matches!(spec, ResourceSpec::Imported { .. }));
    if imports_resource {
        for (type_id, label) in linker_resource_labels(linker) {
            reserved.fallback_resource_name(store, type_id, ResourceType::new(label));
        }
    }

    // One fresh handle table per component instance, shared by every
    // resource type and every other handle kind the instance uses:
    // the canonical ABI keeps handles per instance, and an index of
    // one kind can name an entry of another, which the typed lookups
    // reject. A waitable-set entry takes an index from the same
    // table, so the list covers every instance rather than only the
    // ones that keep a resource.
    let instance_tables: Vec<TableId> = (0..ir.num_component_instances)
        .map(|_| TableId::fresh())
        .collect();
    let resource_tables: Vec<Option<ResourceTableRuntime>> = ir
        .resource_tables
        .iter()
        .map(|spec| {
            spec.as_ref().and_then(|spec| {
                let runtime = resource_runtimes.get(spec.resource_index)?;
                Some(ResourceTableRuntime {
                    table: *instance_tables.get(spec.instance)?,
                    type_id: runtime.type_id,
                    resource_index: spec.resource_index,
                    defining: spec.defining,
                    guest_defined: matches!(runtime.destructor, ResourceDestructor::Local { .. }),
                })
            })
        })
        .collect();

    // One may-leave flag per component instance, minted as the core
    // global the instance's adapter modules import. A fresh
    // instantiation starts with every flag set, because every
    // instance may be left until an adapter is in the middle of
    // translating values across its boundary, or the polyfill is in
    // the middle of a call of its own into the guest. The built-ins
    // read the same globals out of the runtime state below, so the
    // generated code and the polyfill share one flag per instance.
    let flags: Vec<InstanceFlags> = (0..ir.num_component_instances)
        .map(|_| InstanceFlags::new(store.internal().runtime_mut()))
        .collect();

    let abi_state = Arc::new(Mutex::new(
        AbiRuntimeState::with_slabs(
            ir.num_runtime_memories,
            ir.num_runtime_reallocs,
            ir.num_runtime_post_returns,
            ir.num_runtime_callbacks,
            resource_tables,
            component_instances,
            instance_tables,
        )
        .with_instance_flags(flags.clone()),
    ));

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
            &flags,
        )?;
        trampolines.push(func);
    }

    let mut items = RuntimeItems {
        core_instances: Vec::new(),
        trampolines,
        flags,
    };

    for initializer in ir.initializers.iter() {
        match initializer {
            Initializer::InstantiateModule {
                module_index,
                component_instance,
                imports,
            } => {
                let entry = ir
                    .modules
                    .get(*module_index)
                    .ok_or_else(|| internal("module index in IR initializer is out of bounds"))?;
                let runtime_imports = build_imports(ir, &items, store, entry, imports)?;
                // A core module's `start` function runs inside this
                // call, and it is guest code of the component
                // instance the module belongs to. The task it runs
                // in is the current scope for the length of the
                // instantiation, and the instance may not suspend
                // while it does.
                let owner = match component_instance {
                    Some(index) => Some(instance_id_at(&abi_state, *index)?),
                    None => None,
                };
                let start = StartTask::enter(store.internal().tables(), owner)?;
                let instance = RuntimeInstance::new(
                    store.internal().runtime_mut(),
                    entry.module.inner(),
                    &runtime_imports,
                )
                .map_err(InstantiationError::SubstrateFailure)
                .map_err(Error::from);
                drop(start);
                items.core_instances.push(instance?);
            }
            Initializer::InstantiateImportedModule {
                source,
                component_instance,
                imports,
            } => {
                let module = lookup_module(linker, component, resolution, source)?;
                let mut runtime_imports = Imports::default();
                for import in imports.iter() {
                    let value = resolve_source(ir, &items, store, &import.source)?;
                    runtime_imports.define(&import.module, &import.name, value);
                }
                // A host-supplied module's `start` function is guest
                // code of the component instance the module belongs
                // to, exactly as a statically-declared module's is,
                // so it runs in a task of that instance.
                let owner = match component_instance {
                    Some(index) => Some(instance_id_at(&abi_state, *index)?),
                    None => None,
                };
                let start = StartTask::enter(store.internal().tables(), owner)?;
                let instance = RuntimeInstance::new(
                    store.internal().runtime_mut(),
                    module.inner(),
                    &runtime_imports,
                )
                .map_err(InstantiationError::SubstrateFailure)
                .map_err(Error::from);
                drop(start);
                items.core_instances.push(instance?);
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
                let (
                    ResourceSpec::Local { destructor, .. },
                    ResourceDestructor::Local { function: slot, .. },
                ) = (spec, &runtime.destructor)
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
                    let ty = function.ty(store.internal().runtime());
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
            Initializer::ExtractCallback { slot, source } => {
                let extern_value = resolve_source(ir, &items, store, source)?;
                let RuntimeExtern::Func(callback) = extern_value else {
                    return Err(internal(
                        "ExtractCallback directive resolved to a non-function item",
                    ));
                };
                let mut state = abi_state
                    .lock()
                    .map_err(|_| internal("ABI state poisoned"))?;
                if let Some(s) = state.callbacks.get_mut(*slot) {
                    *s = Some(callback);
                } else {
                    return Err(internal("ExtractCallback slot out of bounds"));
                }
            }
        }
    }

    let function_exports = collect_function_exports(ir, &items, store)?;
    let module_exports = collect_module_exports(ir, linker, component, resolution)?;
    Ok(InstanceParts {
        core_instances: items.core_instances.into_boxed_slice(),
        function_exports,
        instance_exports: ir.instance_exports.clone(),
        module_exports,
        abi_state,
        store_id: store.internal().id(),
    }
    .into())
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
    flags: &[InstanceFlags],
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
            let calling = table_instance(component, *table_index)?;
            Ok(build_resource_drop_trampoline(
                store,
                table,
                runtime.clone(),
                instance_flags(flags, calling)?,
            ))
        }
        TrampolineSpec::ResourceNew { table_index } => {
            let table = resource_table(abi_state, *table_index)?;
            let calling = table_instance(component, *table_index)?;
            Ok(build_resource_new_trampoline(
                store,
                table,
                instance_flags(flags, calling)?,
            ))
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
        TrampolineSpec::EndTransfer { tables, signature } => Ok(build_end_transfer(
            store,
            tables.clone(),
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::Trap { signature, code } => build_trap(store, signature, *code),
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
        TrampolineSpec::BackpressureInc {
            instance,
            signature,
        } => Ok(build_backpressure_inc(
            store,
            signature,
            abi_state.clone(),
            *instance,
        )),
        TrampolineSpec::BackpressureDec {
            instance,
            signature,
        } => Ok(build_backpressure_dec(
            store,
            signature,
            abi_state.clone(),
            *instance,
        )),
        TrampolineSpec::TaskReturn {
            result,
            result_tuple,
            options,
            signature,
        } => Ok(build_task_return(
            store,
            result.clone(),
            *result_tuple,
            options,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::WaitableSetNew {
            instance,
            signature,
        } => Ok(build_waitable_set_new(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::WaitableSetWait { options, signature } => Ok(build_waitable_set_wait(
            store,
            options,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::WaitableSetPoll { options, signature } => Ok(build_waitable_set_poll(
            store,
            options,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::WaitableSetDrop {
            instance,
            signature,
        } => Ok(build_waitable_set_drop(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::WaitableJoin {
            instance,
            signature,
        } => Ok(build_waitable_join(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::SubtaskDrop {
            instance,
            signature,
        } => Ok(build_subtask_drop(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::StreamNew {
            instance,
            payload,
            signature,
        } => Ok(build_stream_new(
            store,
            *instance,
            payload.clone(),
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::FutureNew {
            instance,
            payload,
            signature,
        } => Ok(build_future_new(
            store,
            *instance,
            payload.clone(),
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::DropEnd {
            kind,
            instance,
            payload,
            signature,
        } => Ok(build_drop_end(
            store,
            *kind,
            *instance,
            payload.clone(),
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::Copy {
            kind,
            options,
            payload,
            signature,
        } => Ok(build_copy(
            store,
            *kind,
            options,
            payload.clone(),
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::PrepareCall { memory, signature } => Ok(build_prepare_call(
            store,
            *memory,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::SyncStartCall {
            callback,
            signature,
        } => Ok(build_sync_start_call(
            store,
            *callback,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::AsyncStartCall {
            callback,
            post_return,
            signature,
        } => Ok(build_async_start_call(
            store,
            *callback,
            *post_return,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::ThreadYield {
            instance,
            signature,
        } => Ok(build_thread_yield(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::TaskCancel {
            instance,
            signature,
        } => Ok(build_task_cancel(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
        TrampolineSpec::SubtaskCancel {
            instance,
            signature,
        } => Ok(build_subtask_cancel(
            store,
            *instance,
            signature,
            abi_state.clone(),
        )),
    }
}

/// The registration an import resolved to, and the item name to
/// look up in it, or the structured link error when the resolver
/// recorded no registration for it. An interface-named instance
/// import resolves to the chosen interface entry; a plain-named
/// instance import resolves to the nested root entry under its name;
/// either is then walked one nested registration per leading segment
/// of `path`, and the last segment is the item. A function,
/// resource, or module import resolves to the root entry under the
/// name the resolver recorded on the binding — the import's own
/// name, plain or an interface identifier, unless a
/// version-compatible root registration answered it — with an empty
/// `path`.
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
            item: None,
        })
    };
    let mut registration = match resolution.bindings.get(import_index) {
        Some(ImportBinding::Resolved { chosen }) => {
            linker.registration_for(chosen).ok_or_else(unresolved)?
        }
        Some(ImportBinding::Root { name }) => {
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

/// The item a post-resolution lookup names in a diagnostic: the
/// names walked from the import down to the item, joined with dots.
///
/// An empty path means the import is the item — a function,
/// resource, or module import satisfied by the root entry under the
/// import's own name — and such a miss names no item.
fn item_of(path: &[String]) -> Option<String> {
    (!path.is_empty()).then(|| path.join("."))
}

/// Look up a host-resource registration that satisfies the given
/// [`ResourceSpec`]. Mirrors [`lookup_host_func`] but returns the
/// destructor and identity for the resource.
fn resolve_resource_runtime<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    resolution: &Resolution,
    spec: &ResourceSpec,
    component_instances: &[InstanceId],
) -> Result<ResourceRuntime<T>> {
    let (import_index, path) = match spec {
        ResourceSpec::Local { instance, .. } => {
            let instance = *component_instances
                .get(*instance)
                .ok_or_else(|| internal("a locally-defined resource names an unknown instance"))?;
            return Ok(ResourceRuntime::local(instance));
        }
        ResourceSpec::Imported { import_index, path } => (*import_index, path),
    };
    let (registration, label) =
        registration_and_item(linker, component, resolution, import_index, path)?;
    let host = registration.resource(&label).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: component.imports[import_index].name.clone(),
            item: item_of(path),
        })
    })?;
    Ok(ResourceRuntime::from_registration(host, &label))
}

/// Every host resource the linker holds, as the identity it was
/// registered against and the label it was registered under: the
/// root namespace's registrations, then each interface's, then the
/// nested registrations of a plain-named instance import.
///
/// An identity registered under several labels — one host resource
/// value against two interfaces, which is what a shared resource
/// type identity is for — appears once per label. The caller keeps
/// the first it is handed, so the labels are sorted: the order the
/// linker stores its interfaces in is a hash order, and a name a
/// user reads must not depend on it.
fn linker_resource_labels<T: 'static>(linker: &Linker<T>) -> Vec<(ResourceTypeId, String)> {
    fn collect<T: 'static>(
        registration: &InstanceRegistration<T>,
        into: &mut Vec<(ResourceTypeId, String)>,
    ) {
        for (label, resource) in registration.resources.iter() {
            into.push((resource.type_id(), label.clone()));
        }
        for nested in registration.instances.values() {
            collect(nested, into);
        }
    }

    let mut labels = Vec::new();
    collect(linker.root_registration(), &mut labels);
    for key in linker.registered_keys() {
        if let Some(registration) = linker.registration_for(key) {
            collect(registration, &mut labels);
        }
    }
    labels.sort_by(|a, b| a.1.cmp(&b.1));
    labels
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
            .ir()
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
                    item: item_of(path),
                })
            })
        }
    }
}

/// Look up the host-function payload registered against the import
/// the lowering targets. Returns the registration's kind, which
/// carries a handle on the body of whichever of the two registration
/// forms it is, for the trampoline to capture.
fn lookup_host_func<T: 'static>(
    linker: &Linker<T>,
    component: &Component,
    resolution: &Resolution,
    spec: &LoweringSpec,
) -> Result<HostFuncKind<T>> {
    let (registration, item_name) =
        registration_and_item(linker, component, resolution, spec.import_index, &spec.path)?;
    let host = registration.func(&item_name).ok_or_else(|| {
        Error::from(LinkError::UnresolvedImport {
            import: component.imports[spec.import_index].name.clone(),
            item: item_of(&spec.path),
        })
    })?;
    Ok(host.kind.clone())
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
            .map(|flags| RuntimeExtern::Global(flags.global().clone()))
            .ok_or_else(|| internal("ImportSource::InstanceFlags index is out of bounds")),
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
        .get_export(store.internal().runtime(), name)
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
) -> Result<Box<[Arc<ExportedFunction>]>> {
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
        out.push(Arc::new(ExportedFunction {
            name: name.clone(),
            path: path.clone(),
            func,
            signature: Arc::clone(signature),
            options: Arc::clone(options),
        }));
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

/// The store-wide identity of the component instance the
/// translator's per-instantiation `index` names.
fn instance_id_at(abi_state: &Arc<Mutex<AbiRuntimeState>>, index: usize) -> Result<InstanceId> {
    let state = abi_state
        .lock()
        .map_err(|_| internal("ABI state poisoned"))?;
    state
        .component_instances
        .get(index)
        .copied()
        .ok_or_else(|| internal("an initializer names a component instance the plan does not hold"))
}

/// The translator's index of the component instance that keeps the
/// resource table at `table_index`, which is the instance whose core
/// code calls the `resource.new` or `resource.drop` built-in the
/// table belongs to.
fn table_instance(component: &Component, table_index: usize) -> Result<usize> {
    component
        .ir()
        .resource_tables
        .get(table_index)
        .and_then(|spec| spec.as_ref())
        .map(|spec| spec.instance)
        .ok_or_else(|| {
            internal("resource trampoline names a table the instantiation does not hold")
        })
}

/// The may-leave flag of the component instance at `index`.
fn instance_flags(flags: &[InstanceFlags], index: usize) -> Result<InstanceFlags> {
    flags
        .get(index)
        .cloned()
        .ok_or_else(|| internal("a component instance of the plan has no may-leave flag"))
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
