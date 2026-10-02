// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Component translator.
//!
//! Drives [`wasmtime_environ::component::Translator`] over a
//! component binary exactly once, compiles every core module the
//! binary contains against the runtime layer, projects the
//! translator's type information onto the polyfill's data shapes,
//! and projects the translator's rich IR into the polyfill's own
//! [`ExecutorIr`] so the executor's driver is target-agnostic. The
//! same translator runs on every supported target; see [`super`]
//! for why the `compile` feature builds cleanly on
//! `wasm32-unknown-unknown`.

use std::collections::HashMap;
use std::sync::Arc;

use wasmtime_environ::component::{
    CanonicalOptions as EnvironCanonOptions, CanonicalOptionsDataModel, ComponentTranslation,
    ComponentTypes, ComponentTypesBuilder, CoreDef, CoreExport, Export as EnvironExport,
    ExportIndex, ExportItem as EnvironExportItem, ExtractCallback, ExtractMemory,
    ExtractPostReturn, ExtractRealloc, ExtractTable, FixedEncoding, GlobalInitializer,
    InstantiateModule, InterfaceType, LoweredIndex, OptionsIndex, RuntimeImportIndex,
    StaticModuleIndex, StringEncoding as EnvironStringEncoding, Trampoline, TrampolineIndex,
    Transcode, Translator, TypeComponentLocalErrorContextTableIndex, TypeFutureTableIndex,
    TypeResourceTable, TypeResourceTableIndex, TypeStreamTableIndex, UnsafeIntrinsic,
};
use wasmtime_environ::prelude::Error as TranslatorError;
use wasmtime_environ::wasmparser::{Validator, WasmFeatures};
use wasmtime_environ::{
    EntityIndex as EnvironEntityIndex, ScopeVec, Tunables, WasmError, WasmHeapType, WasmValType,
};

use crate::abi::layout::FlatType;
use crate::abi::signature::Signature;
use crate::internal::{EngineConfigInternal, EngineInternal, ErrorInternal, ModuleInternal};

use crate::component::{ComponentExport, ComponentImport, ExternType, ExternalName, TypeProjector};
use crate::concurrency::{EndKind, LowerKind};
use crate::engine::Engine;
use crate::error::{Error, Result};

use crate::module::Module;
use crate::runtime_layer::{Capabilities, Capability};
use crate::types::{PrimitiveType, ValueType};

use super::ir::{
    CanonOptions, CoreInstanceExport, CoreParameter, CoreSignature, CoreSourceItem, DataModel,
    EndTableSpec, EntityIndex, ExecutorIr, ExportSpec, ImportSource, Initializer, LoweringSpec,
    ModuleEntry, ModuleExportSpec, ModuleSource, NamedImportSource, ResourceSpec,
    ResourceTableSpec, StringEncoding, TrampolineSpec, TranscodeOp,
};
use super::thread_start_table::THREAD_START_PROBE;
use super::translator_features::{
    missing_capability, require_single_memories, translator_features,
};
use super::{compile_module, compile_modules};

/// The slot the [`TrampolineSpec`] of each trampoline of a component
/// landed in, by the translator's index.
type TrampolineSlots = HashMap<TrampolineIndex, usize>;

/// Everything one translation of a component binary produces.
pub struct Translation {
    /// The declared imports, in declaration order.
    pub imports: Box<[ComponentImport]>,
    /// The declared exports, in declaration order.
    pub exports: Box<[ComponentExport]>,
    /// The executor's plan, with every core module compiled.
    pub ir: ExecutorIr,
}

/// Translate `bytes` against `engine`. The future suspends only while
/// the browser compiles a core module; on native it completes without
/// suspending.
pub async fn translate(engine: &Engine, bytes: &[u8]) -> Result<Translation> {
    let scope = ScopeVec::new();
    // The translator's defaults keep concurrency support on. Turning
    // it off makes the fused adapter compiler assert on an `async`
    // function instead of reporting it, so the polyfill leaves it on
    // and provides the enter- and exit-sync-call intrinsics an adapter
    // imports under that setting.
    let tunables = Tunables::default_u32();
    // The features follow the capabilities of the backend, so the
    // translator accepts only core code the backend can compile, and
    // its adapters carry an exception barrier only where the backend
    // declares exception handling.
    let capabilities = engine.inner().capabilities();
    let features = translator_features(engine.config().wasm_features(), capabilities);
    let mut validator = Validator::new_with_features(features);
    let mut types = ComponentTypesBuilder::new(&validator);

    let (translation, modules) = Translator::new(&tunables, &mut validator, &mut types, &scope)
        .translate(bytes)
        .map_err(|err| refusal(err, bytes, features, capabilities))?;
    // The translator ran with multi-memory on for its adapters. A
    // module with two memories, over a backend without them, fails
    // here, before any module compiles.
    require_single_memories(
        capabilities,
        modules.values().map(|module| module.module.memories.len()),
    )?;

    // The builder knows how many resource, stream, future, and
    // error-context tables the component has; the finished types
    // index them but do not count them.
    let num_resource_tables = types.num_resource_tables();
    let num_stream_tables = types.num_stream_tables();
    let num_future_tables = types.num_future_tables();
    let num_error_context_tables = types.num_error_context_tables();
    let (component_types, _) = types.finish(&translation.component);
    let projector = TypeProjector::new(&component_types, &translation.component);

    let imports = project_imports(&translation, &projector)?;
    let exports = project_exports(&translation, &projector)?;

    let mut module_entries: Vec<ModuleEntry> = Vec::with_capacity(modules.len());
    let mut module_index_for_static: HashMap<StaticModuleIndex, usize> =
        HashMap::with_capacity(modules.len());
    // Every module is compiled before any is wrapped, so the browser
    // is handed all of its asynchronous compiles at once rather than
    // one after the other finishes.
    let binaries: Vec<&[u8]> = modules.values().map(|module| module.wasm).collect();
    let compiled_modules = compile_modules(engine, &binaries).await?;
    for ((static_idx, module), compiled) in modules.into_iter().zip(compiled_modules) {
        let compiled = Module::from_compiled(compiled, module.wasm)?;
        let entity_to_name = module
            .module
            .exports
            .iter()
            .map(|(atom, idx)| {
                (
                    lift_entity_index(*idx),
                    module.module.strings[*atom].to_owned(),
                )
            })
            .collect();
        module_index_for_static.insert(static_idx, module_entries.len());
        module_entries.push(ModuleEntry {
            module: compiled,
            entity_to_name,
        });
    }

    // Pre-walk #1: build `LoweredIndex → RuntimeImportIndex` from
    // the initializer list. The trampoline pre-walk consults this
    // map to pair its options with the right import.
    let mut lowered_to_import: HashMap<LoweredIndex, RuntimeImportIndex> = HashMap::new();
    for initializer in &translation.component.initializers {
        if let GlobalInitializer::LowerImport { index, import } = initializer {
            lowered_to_import.insert(*index, *import);
        }
    }

    // Pre-walk #2: one `ResourceSpec` per resource, at the
    // translator's resource index: imported resources first, then the
    // ones the component defines. A defined resource's destructor is
    // filled in when its initializer is reached below.
    let mut resources: Vec<ResourceSpec> = Vec::new();
    for (_, runtime_import) in translation.component.imported_resources.iter() {
        let (import_index, path) = import_path(&translation, *runtime_import)?;
        resources.push(ResourceSpec::Imported { import_index, path });
    }
    for (_, instance) in translation.component.defined_resource_instances.iter() {
        resources.push(ResourceSpec::Local {
            instance: instance.as_u32() as usize,
            destructor: None,
        });
    }

    // Pre-walk #2b: one entry per resource table, at the translator's
    // table index. A concrete table names its resource and the instance
    // that keeps it; an abstract one has no runtime presence.
    let resource_tables: Vec<Option<ResourceTableSpec>> = (0..num_resource_tables as u32)
        .map(
            |i| match &component_types[TypeResourceTableIndex::from_u32(i)] {
                TypeResourceTable::Concrete { ty, instance } => {
                    let defining = translation
                        .component
                        .defined_resource_index(*ty)
                        .map(|defined| {
                            translation.component.defined_resource_instances[defined] == *instance
                        })
                        .unwrap_or(false);
                    Some(ResourceTableSpec {
                        resource_index: ty.as_u32() as usize,
                        instance: instance.as_u32() as usize,
                        defining,
                        resource_type: projector.resource(TypeResourceTableIndex::from_u32(i)),
                    })
                }
                TypeResourceTable::Abstract(_) => None,
            },
        )
        .collect();

    // Pre-walk #3: build every trampoline. Each gets a
    // `TrampolineSpec` and the slot it landed in, which
    // `CoreDef::Trampoline` resolution reads.
    let mut trampoline_specs: Vec<TrampolineSpec> = Vec::new();
    let mut trampolines: TrampolineSlots = HashMap::new();
    let mut stream_tables: Option<Arc<[EndTableSpec]>> = None;
    let mut future_tables: Option<Arc<[EndTableSpec]>> = None;
    for (trampoline_idx, trampoline) in translation.trampolines.iter() {
        let spec = match trampoline {
            Trampoline::LowerImport {
                index: lowered_idx,
                lower_ty,
                options,
            } => {
                let runtime_import = *lowered_to_import.get(lowered_idx).ok_or_else(|| {
                    Error::internal(
                        "Trampoline::LowerImport has no matching GlobalInitializer::LowerImport",
                    )
                })?;
                let (import_index, path) = import_path(&translation, runtime_import)?;
                let signature = projector.function(*lower_ty)?;
                let options = trampoline_options(&translation, *options)?;
                let kind = lower_kind(&options);
                TrampolineSpec::LowerImport(LoweringSpec {
                    import_index,
                    path,
                    signature: Arc::new(Signature::new(signature)),
                    options: Arc::new(options),
                    kind,
                })
            }
            Trampoline::ResourceDrop { ty, .. } => TrampolineSpec::ResourceDrop {
                table_index: resolve_table_index(&component_types, resource_tables.len(), *ty)?,
            },
            Trampoline::ResourceNew { ty, .. } => TrampolineSpec::ResourceNew {
                table_index: resolve_table_index(&component_types, resource_tables.len(), *ty)?,
            },
            Trampoline::ResourceRep { ty, .. } => TrampolineSpec::ResourceRep {
                table_index: resolve_table_index(&component_types, resource_tables.len(), *ty)?,
            },
            // A 64-bit memory on either side shows in the core
            // signature the translator recorded: the transcoder reads
            // its pointers and lengths at the width of each slot.
            Trampoline::Transcoder { op, from, to, .. } => TrampolineSpec::Transcoder {
                op: lift_transcode_op(*op),
                from_memory: from.as_u32() as usize,
                to_memory: to.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ResourceTransferOwn => TrampolineSpec::ResourceTransferOwn {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ResourceTransferBorrow => TrampolineSpec::ResourceTransferBorrow {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The two transfer intrinsics of a readable end. Each
            // names its source and destination by the translator's
            // stream or future table index, so the trampoline carries
            // every table of its kind. The tables are projected the
            // first time a component needs them, so a component that
            // transfers no end never projects a stream or future type.
            Trampoline::StreamTransfer => TrampolineSpec::EndTransfer {
                tables: match &stream_tables {
                    Some(tables) => tables.clone(),
                    None => stream_tables
                        .insert(end_tables(num_stream_tables, |i| {
                            let index = TypeStreamTableIndex::from_u32(i);
                            Ok(EndTableSpec {
                                instance: component_types[index].instance.as_u32() as usize,
                                ty: projector.value_type(&InterfaceType::Stream(index))?,
                            })
                        })?)
                        .clone(),
                },
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::FutureTransfer => TrampolineSpec::EndTransfer {
                tables: match &future_tables {
                    Some(tables) => tables.clone(),
                    None => future_tables
                        .insert(end_tables(num_future_tables, |i| {
                            let index = TypeFutureTableIndex::from_u32(i);
                            Ok(EndTableSpec {
                                instance: component_types[index].instance.as_u32() as usize,
                                ty: projector.value_type(&InterfaceType::Future(index))?,
                            })
                        })?)
                        .clone(),
                },
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The transfer intrinsic of an error context names its
            // source and destination by the translator's error-context
            // table index, and each such table is the handle table of
            // one component instance.
            Trampoline::ErrorContextTransfer => TrampolineSpec::ErrorContextTransfer {
                instances: (0..num_error_context_tables as u32)
                    .map(|i| {
                        let index = TypeComponentLocalErrorContextTableIndex::from_u32(i);
                        component_types[index].instance.as_u32() as usize
                    })
                    .collect(),
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::Trap(trap) => TrampolineSpec::Trap {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
                code: *trap as u8,
            },
            Trampoline::EnterSyncCall => TrampolineSpec::EnterSyncCall {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ExitSyncCall => TrampolineSpec::ExitSyncCall {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The two backpressure built-ins name the instance whose
            // counter they move. The guest imports them directly, so
            // the instance comes from the trampoline rather than from
            // an argument.
            Trampoline::BackpressureInc { instance } => TrampolineSpec::BackpressureInc {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::BackpressureDec { instance } => TrampolineSpec::BackpressureDec {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The result values are the built-in's parameters, so the
            // core signature the translator recorded already carries
            // the flat-parameter rule: the flattened result in slots,
            // or one pointer beyond sixteen of them.
            Trampoline::TaskReturn {
                results, options, ..
            } => TrampolineSpec::TaskReturn {
                result: projector.result_tuple(*results)?,
                result_tuple: results.as_u32() as usize,
                options: lift_canon_options(canon_options(&translation, *options)?)?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The waitable set built-ins. Each one reaches the set
            // records of the store through the handle table of the
            // instance the translator names here; the two that
            // deliver an event also name the memory its payloads are
            // written through, which their canon options carry.
            Trampoline::WaitableSetNew { instance } => TrampolineSpec::WaitableSetNew {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The two that deliver an event keep the `cancellable`
            // immediate their canon options carry, as the thread
            // built-ins below keep theirs.
            Trampoline::WaitableSetWait { options, .. } => TrampolineSpec::WaitableSetWait {
                options: trampoline_options(&translation, *options)?,
                cancellable: trampoline_cancellable(&translation, *options)?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::WaitableSetPoll { options, .. } => TrampolineSpec::WaitableSetPoll {
                options: trampoline_options(&translation, *options)?,
                cancellable: trampoline_cancellable(&translation, *options)?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::WaitableSetDrop { instance } => TrampolineSpec::WaitableSetDrop {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::WaitableJoin { instance } => TrampolineSpec::WaitableJoin {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The one subtask built-in the polyfill implements. It
            // names the instance whose handle table holds the entry,
            // as the waitable built-ins beside it do.
            Trampoline::SubtaskDrop { instance } => TrampolineSpec::SubtaskDrop {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The built-ins that create and drop the ends of a stream
            // or a future. Each names the instance whose handle table
            // holds the ends, and the stream or future type it was
            // declared with, whose payload the ends carry.
            Trampoline::StreamNew { instance, ty } => TrampolineSpec::StreamNew {
                instance: instance.as_u32() as usize,
                payload: payload_of(&projector, InterfaceType::Stream(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::FutureNew { instance, ty } => TrampolineSpec::FutureNew {
                instance: instance.as_u32() as usize,
                payload: payload_of(&projector, InterfaceType::Future(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::StreamDropReadable { instance, ty } => TrampolineSpec::DropEnd {
                kind: EndKind::StreamReadable,
                instance: instance.as_u32() as usize,
                payload: payload_of(&projector, InterfaceType::Stream(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::StreamDropWritable { instance, ty } => TrampolineSpec::DropEnd {
                kind: EndKind::StreamWritable,
                instance: instance.as_u32() as usize,
                payload: payload_of(&projector, InterfaceType::Stream(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::FutureDropReadable { instance, ty } => TrampolineSpec::DropEnd {
                kind: EndKind::FutureReadable,
                instance: instance.as_u32() as usize,
                payload: payload_of(&projector, InterfaceType::Future(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::FutureDropWritable { instance, ty } => TrampolineSpec::DropEnd {
                kind: EndKind::FutureWritable,
                instance: instance.as_u32() as usize,
                payload: payload_of(&projector, InterfaceType::Future(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The copies of a stream and of a future. Each carries its
            // canon options, which name the calling instance, the
            // `async` flag, and the memory the guest's buffer lives
            // in, and the path its copies take, which the payload
            // type selects.
            Trampoline::StreamRead { ty, options, .. } => copy_spec(
                EndKind::StreamReadable,
                trampoline_options(&translation, *options)?,
                payload_of(&projector, InterfaceType::Stream(*ty))?,
                core_signature(&component_types, &translation, trampoline_idx)?,
            ),
            Trampoline::StreamWrite { ty, options, .. } => copy_spec(
                EndKind::StreamWritable,
                trampoline_options(&translation, *options)?,
                payload_of(&projector, InterfaceType::Stream(*ty))?,
                core_signature(&component_types, &translation, trampoline_idx)?,
            ),
            Trampoline::FutureRead { ty, options, .. } => copy_spec(
                EndKind::FutureReadable,
                trampoline_options(&translation, *options)?,
                payload_of(&projector, InterfaceType::Future(*ty))?,
                core_signature(&component_types, &translation, trampoline_idx)?,
            ),
            Trampoline::FutureWrite { ty, options, .. } => copy_spec(
                EndKind::FutureWritable,
                trampoline_options(&translation, *options)?,
                payload_of(&projector, InterfaceType::Future(*ty))?,
                core_signature(&component_types, &translation, trampoline_idx)?,
            ),
            // The cancels of a copy on a stream and on a future. Each
            // names the calling instance and the type it was declared
            // with, and carries its `async` flag, which decides whether
            // a cancel that has not finished waits or reports the
            // copy blocked.
            Trampoline::StreamCancelRead {
                instance,
                ty,
                async_,
            } => TrampolineSpec::CancelCopy {
                kind: EndKind::StreamReadable,
                instance: instance.as_u32() as usize,
                async_: *async_,
                payload: payload_of(&projector, InterfaceType::Stream(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::StreamCancelWrite {
                instance,
                ty,
                async_,
            } => TrampolineSpec::CancelCopy {
                kind: EndKind::StreamWritable,
                instance: instance.as_u32() as usize,
                async_: *async_,
                payload: payload_of(&projector, InterfaceType::Stream(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::FutureCancelRead {
                instance,
                ty,
                async_,
            } => TrampolineSpec::CancelCopy {
                kind: EndKind::FutureReadable,
                instance: instance.as_u32() as usize,
                async_: *async_,
                payload: payload_of(&projector, InterfaceType::Future(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::FutureCancelWrite {
                instance,
                ty,
                async_,
            } => TrampolineSpec::CancelCopy {
                kind: EndKind::FutureWritable,
                instance: instance.as_u32() as usize,
                async_: *async_,
                payload: payload_of(&projector, InterfaceType::Future(*ty))?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The prepare-and-start pair of a fused adapter whose
            // lower or lift is asynchronous. Prepare names the
            // memory the callee's lift declared, which the callee's
            // `task.return` has to name too; everything else about
            // the call travels as an argument, the two generated
            // functions as `funcref`s among them.
            Trampoline::PrepareCall { memory } => TrampolineSpec::PrepareCall {
                memory: memory.map(|slot| slot.as_u32() as usize),
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // A synchronous start reaches an asynchronously lifted
            // callee alone. The callback is what the callee's status
            // word is handed to. A lift that named none is the
            // stackful form, whose core function returns nothing.
            Trampoline::SyncStartCall { callback } => TrampolineSpec::SyncStartCall {
                callback: callback.map(|slot| slot.as_u32() as usize),
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // An asynchronous start reaches all three lifts, and the
            // adapter says which at the call rather than here: a
            // callback slot alone does not tell a synchronous lift
            // from the stackful form, since neither names one.
            Trampoline::AsyncStartCall {
                callback,
                post_return,
            } => TrampolineSpec::AsyncStartCall {
                callback: callback.map(|slot| slot.as_u32() as usize),
                post_return: post_return.map(|slot| slot.as_u32() as usize),
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The `thread.yield` built-in. It keeps the `cancellable`
            // field of the trampoline IR, and so do the five thread
            // built-ins that suspend or switch below. The reference
            // removed the immediate, and the release after this one
            // drops the field, but Wasmtime 49 honors it and the C
            // generator of wit-bindgen still emits it, so one guest
            // binary behaves the same in both: a cancellable built-in
            // takes a pending cancellation request and answers 1.
            Trampoline::ThreadYield {
                instance,
                cancellable,
            } => TrampolineSpec::ThreadYield {
                instance: instance.as_u32() as usize,
                cancellable: *cancellable,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The three thread built-ins that never switch stacks.
            // Each names the instance whose thread table it works on.
            // `thread.new-indirect` also names the runtime table its
            // start function is read from, which a table initializer
            // below extracts. The start function type it declares is
            // dropped: the context parameter of the core signature
            // is of the one type the start function takes, and the
            // type index names a type in the declaring component's
            // own index space, which inlining has left behind.
            Trampoline::ThreadIndex { instance } => TrampolineSpec::ThreadIndex {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ThreadNewIndirect {
                instance,
                start_func_table_idx,
                ..
            } => TrampolineSpec::ThreadNewIndirect {
                instance: instance.as_u32() as usize,
                table: start_func_table_idx.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ThreadResumeLater { instance } => TrampolineSpec::ThreadResumeLater {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The five thread built-ins that suspend or switch. Each
            // names the instance whose thread table it works on, and
            // keeps its `cancellable` field, for the reason the
            // `thread.yield` arm gives.
            Trampoline::ThreadSuspend {
                instance,
                cancellable,
            } => TrampolineSpec::ThreadSuspend {
                instance: instance.as_u32() as usize,
                cancellable: *cancellable,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ThreadSuspendThenResume {
                instance,
                cancellable,
            } => TrampolineSpec::ThreadSuspendThenResume {
                instance: instance.as_u32() as usize,
                cancellable: *cancellable,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ThreadYieldThenResume {
                instance,
                cancellable,
            } => TrampolineSpec::ThreadYieldThenResume {
                instance: instance.as_u32() as usize,
                cancellable: *cancellable,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ThreadSuspendThenPromote {
                instance,
                cancellable,
            } => TrampolineSpec::ThreadSuspendThenPromote {
                instance: instance.as_u32() as usize,
                cancellable: *cancellable,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ThreadYieldThenPromote {
                instance,
                cancellable,
            } => TrampolineSpec::ThreadYieldThenPromote {
                instance: instance.as_u32() as usize,
                cancellable: *cancellable,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The three error-context built-ins. The validator admits
            // them only under the engine's error-context gate, so a
            // component that reaches these arms opted in. The two
            // that move a debug message carry their canon options,
            // which name the calling instance, the memory, and the
            // string encoding, and for `debug-message` the `realloc`.
            // The type index each names is dropped: an error context
            // has no type beyond its kind, and the entry lives in
            // the instance's one handle table.
            Trampoline::ErrorContextNew { options, .. } => TrampolineSpec::ErrorContextNew {
                options: trampoline_options(&translation, *options)?,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ErrorContextDebugMessage { options, .. } => {
                TrampolineSpec::ErrorContextDebugMessage {
                    options: trampoline_options(&translation, *options)?,
                    signature: core_signature(&component_types, &translation, trampoline_idx)?,
                }
            }
            Trampoline::ErrorContextDrop { instance, .. } => TrampolineSpec::ErrorContextDrop {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // The two cancellation built-ins. Each names the instance
            // that calls it, and `subtask.cancel` keeps its `async`
            // immediate, which decides whether it blocks.
            Trampoline::TaskCancel { instance } => TrampolineSpec::TaskCancel {
                instance: instance.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::SubtaskCancel { instance, async_ } => TrampolineSpec::SubtaskCancel {
                instance: instance.as_u32() as usize,
                async_: *async_,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
        };
        let slot = trampoline_specs.len();
        trampoline_specs.push(spec);
        trampolines.insert(trampoline_idx, slot);
    }

    // Main walk. Intrinsics that appear as `CoreDef`s rather than
    // as trampolines get their specs appended after the pre-built
    // ones, so the state knows where its own entries start.
    let mut state = ProjectionState::new(trampoline_specs.len());

    for initializer in &translation.component.initializers {
        match initializer {
            GlobalInitializer::InstantiateModule(
                InstantiateModule::Static(static_idx, defs),
                component_instance,
            ) => {
                let module_index = *module_index_for_static.get(static_idx).ok_or_else(|| {
                    Error::internal("module index from translator missing from projection map")
                })?;
                let mut imports = Vec::with_capacity(defs.len());
                for def in defs.iter() {
                    imports.push(state.lift_core_def(def, &trampolines)?);
                }
                state.runtime_instance_to_module.push(Some(module_index));
                state.initializers.push(Initializer::InstantiateModule {
                    module_index,
                    component_instance: component_instance.map(|i| i.as_u32() as usize),
                    imports: imports.into_boxed_slice(),
                });
            }
            GlobalInitializer::InstantiateModule(
                InstantiateModule::Import(import, args),
                component_instance,
            ) => {
                let (import_index, path) = import_path(&translation, *import)?;
                let mut imports = Vec::new();
                for (module, items) in args.iter() {
                    for (name, def) in items.iter() {
                        imports.push(NamedImportSource {
                            module: module.clone(),
                            name: name.clone(),
                            source: state.lift_core_def(def, &trampolines)?,
                        });
                    }
                }
                state.runtime_instance_to_module.push(None);
                state
                    .initializers
                    .push(Initializer::InstantiateImportedModule {
                        source: ModuleSource::Import { import_index, path },
                        component_instance: component_instance.map(|i| i.as_u32() as usize),
                        imports: imports.into_boxed_slice(),
                    });
            }
            GlobalInitializer::LowerImport { .. } => {
                // The trampoline's runtime-layer function is built
                // upfront from `ir.trampoline_specs` before the
                // initializer walk runs (see `executor::instantiate`).
                // The marker initializer adds no further state.
            }
            GlobalInitializer::ExtractMemory(ExtractMemory { index, export }) => {
                let source = state.lift_core_export(export)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_memories = state.num_runtime_memories.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractMemory { slot, source });
            }
            GlobalInitializer::ExtractRealloc(ExtractRealloc { index, def }) => {
                let source = state.lift_core_def(def, &trampolines)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_reallocs = state.num_runtime_reallocs.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractRealloc { slot, source });
            }
            GlobalInitializer::ExtractPostReturn(ExtractPostReturn { index, def }) => {
                let source = state.lift_core_def(def, &trampolines)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_post_returns = state.num_runtime_post_returns.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractPostReturn { slot, source });
            }
            GlobalInitializer::ExtractCallback(ExtractCallback { index, def }) => {
                let source = state.lift_core_def(def, &trampolines)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_callbacks = state.num_runtime_callbacks.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractCallback { slot, source });
            }
            GlobalInitializer::ExtractTable(ExtractTable { index, export }) => {
                let source = state.lift_core_export(export)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_tables = state.num_runtime_tables.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractTable { slot, source });
            }
            GlobalInitializer::Resource(resource) => {
                if resource.rep != WasmValType::I32 {
                    return Err(Error::unsupported(
                        "resource representations other than i32",
                    ));
                }
                let slot = translation
                    .component
                    .resource_index(resource.index)
                    .as_u32() as usize;
                let destructor = resource
                    .dtor
                    .as_ref()
                    .map(|def| state.lift_core_def(def, &trampolines))
                    .transpose()?;
                match resources.get_mut(slot) {
                    Some(ResourceSpec::Local {
                        destructor: slot_destructor,
                        ..
                    }) => *slot_destructor = destructor,
                    _ => {
                        return Err(Error::internal(
                            "resource initializer names a resource the pre-walk did not define",
                        ));
                    }
                }
                state.initializers.push(Initializer::DefineResource {
                    resource_index: slot,
                });
            }
        }
    }

    let mut export_tree = ExportTree {
        functions: Vec::new(),
        instances: Vec::new(),
        modules: Vec::new(),
        module_index_for_static: &module_index_for_static,
    };
    for (name, (export_index, _)) in translation.component.exports.raw_iter() {
        collect_export_spec(
            &translation,
            &projector,
            &mut state,
            &trampolines,
            &mut export_tree,
            &[],
            name,
            *export_index,
        )?;
    }

    trampoline_specs.append(&mut state.extra_specs);

    // The probe a `thread.new-indirect` reads its start function
    // through is compiled once per component that extracts a table
    // for one, where the backend declares `gc`, which the probe
    // needs; see `ThreadStartTable` for what it answers, and for how
    // the host reads the table without it.
    let thread_start_probe =
        if state.num_runtime_tables > 0 && capabilities.contains(Capability::Gc) {
            Some(crate::runtime_layer::Shared::new(
                compile_module(engine, THREAD_START_PROBE).await?,
            ))
        } else {
            None
        };

    Ok(Translation {
        imports,
        exports,
        ir: ExecutorIr {
            modules: module_entries.into_boxed_slice(),
            initializers: state.initializers.into_boxed_slice(),
            exports: export_tree.functions.into_boxed_slice(),
            instance_exports: export_tree.instances.into_boxed_slice(),
            module_exports: export_tree.modules.into_boxed_slice(),
            trampoline_specs: trampoline_specs.into_boxed_slice(),
            resources: resources.into_boxed_slice(),
            resource_tables: resource_tables.into_boxed_slice(),
            runtime_instance_to_module: state.runtime_instance_to_module.into_boxed_slice(),
            num_runtime_memories: state.num_runtime_memories,
            num_runtime_reallocs: state.num_runtime_reallocs,
            num_runtime_post_returns: state.num_runtime_post_returns,
            num_runtime_callbacks: state.num_runtime_callbacks,
            num_runtime_tables: state.num_runtime_tables,
            thread_start_probe,
            num_component_instances: translation.component.num_runtime_component_instances as usize,
        },
    })
}

/// The canon options a `canon` definition declared, which the
/// translator records once per definition and its trampoline names
/// by index.
fn canon_options(
    translation: &ComponentTranslation,
    index: OptionsIndex,
) -> Result<&EnvironCanonOptions> {
    translation
        .component
        .options
        .get(index)
        .ok_or_else(|| Error::internal("Trampoline OptionsIndex out of bounds"))
}

/// The core signature the translator recorded for a trampoline.
fn core_signature(
    component_types: &ComponentTypes,
    translation: &ComponentTranslation,
    index: TrampolineIndex,
) -> Result<CoreSignature> {
    let interned = *translation
        .component
        .trampolines
        .get(index)
        .ok_or_else(|| Error::internal("trampoline index has no core type"))?;
    let func = component_types.module_types()[interned].unwrap_func();
    let mut params = Vec::with_capacity(func.params().len());
    for ty in func.params() {
        params.push(lift_core_parameter(ty)?);
    }
    let mut results = Vec::with_capacity(func.results().len());
    for ty in func.results() {
        results.push(lift_core_val_type(ty)?);
    }
    Ok(CoreSignature { params, results })
}

/// Project one parameter of an intrinsic's core signature. A
/// `funcref` is a parameter of the prepare-and-start intrinsics
/// alone, which carry the two functions the adapter generated for a
/// call and the callee's own core function; every other reference
/// type is refused.
fn lift_core_parameter(ty: &WasmValType) -> Result<CoreParameter> {
    match ty {
        WasmValType::Ref(reference) if reference.heap_type == WasmHeapType::Func => {
            Ok(CoreParameter::FuncRef)
        }
        other => lift_core_val_type(other).map(CoreParameter::Value),
    }
}

fn lift_core_val_type(ty: &WasmValType) -> Result<FlatType> {
    Ok(match ty {
        WasmValType::I32 => FlatType::I32,
        WasmValType::I64 => FlatType::I64,
        WasmValType::F32 => FlatType::F32,
        WasmValType::F64 => FlatType::F64,
        WasmValType::V128 | WasmValType::Ref(_) => {
            return Err(Error::unsupported(
                "vector or reference types in intrinsic signatures",
            ));
        }
    })
}

fn lift_transcode_op(op: Transcode) -> TranscodeOp {
    match op {
        Transcode::Copy(FixedEncoding::Utf8) => TranscodeOp::CopyUtf8,
        Transcode::Copy(FixedEncoding::Utf16) => TranscodeOp::CopyUtf16,
        Transcode::Copy(FixedEncoding::Latin1) => TranscodeOp::CopyLatin1,
        Transcode::Latin1ToUtf16 => TranscodeOp::Latin1ToUtf16,
        Transcode::Latin1ToUtf8 => TranscodeOp::Latin1ToUtf8,
        Transcode::Utf16ToCompactProbablyUtf16 => TranscodeOp::Utf16ToCompactProbablyUtf16,
        Transcode::Utf16ToCompactUtf16 => TranscodeOp::Utf16ToCompactUtf16,
        Transcode::Utf16ToLatin1 => TranscodeOp::Utf16ToLatin1,
        Transcode::Utf16ToUtf8 => TranscodeOp::Utf16ToUtf8,
        Transcode::Utf8ToCompactUtf16 => TranscodeOp::Utf8ToCompactUtf16,
        Transcode::Utf8ToLatin1 => TranscodeOp::Utf8ToLatin1,
        Transcode::Utf8ToUtf16 => TranscodeOp::Utf8ToUtf16,
    }
}

/// Project the component's declared imports, in declaration order.
/// The position of each entry is the `import_index` the executor's
/// specs refer to.
fn project_imports(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
) -> Result<Box<[ComponentImport]>> {
    let mut imports = Vec::with_capacity(translation.component.import_types.len());
    for (_, (name, extern_)) in translation.component.import_types.iter() {
        let ty = projector.extern_type(name, extern_)?;
        imports.push(ComponentImport {
            name: ExternalName::from_raw(name),
            ty,
        });
    }
    Ok(imports.into_boxed_slice())
}

/// Which lowering a `canon lower` declared, from the `async` option
/// of its canon options.
///
/// The `memory` option is not this function's business. The
/// reference requires it where the canonical ABI loads or stores,
/// and validation — which the translator runs over the binary before
/// any of this — already holds an asynchronous lower to exactly that
/// rule: it refuses one whose parameter tuple transitively contains
/// a pointer, one whose flattened tuple exceeds the four slots such
/// a lower has and therefore spills through one, and one whose type
/// has a result, which always comes back through a return-area
/// pointer. What is left is the shape that addresses no memory at
/// all — at most four flat parameters and no result — and that shape
/// is valid without the option, so the kind is read here and nothing
/// is refused.
fn lower_kind(options: &CanonOptions) -> LowerKind {
    if options.async_ {
        LowerKind::Async
    } else {
        LowerKind::Sync
    }
}

/// Project the component's declared exports, in declaration order.
fn project_exports(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
) -> Result<Box<[ComponentExport]>> {
    let mut exports = Vec::new();
    for (name, (export_index, _)) in translation.component.exports.raw_iter() {
        let ty = match &translation.component.export_items[*export_index] {
            EnvironExport::LiftedFunction { ty, .. } => {
                ExternType::Function(projector.function(*ty)?)
            }
            EnvironExport::Instance { ty, .. } => ExternType::Instance(projector.instance(*ty)?),
            EnvironExport::Type(def) => projector.type_def(name, def)?,
            EnvironExport::ModuleStatic { ty, .. } | EnvironExport::ModuleImport { ty, .. } => {
                ExternType::Module(projector.module(*ty)?)
            }
        };
        exports.push(ComponentExport {
            name: ExternalName::from_raw(name),
            ty,
        });
    }
    Ok(exports.into_boxed_slice())
}

/// The export entries the translator collects: one [`ExportSpec`]
/// per lifted function at any depth, the path of every instance-typed
/// export, and one [`ModuleExportSpec`] per module-typed export.
struct ExportTree<'a> {
    functions: Vec<ExportSpec>,
    instances: Vec<Box<[ExternalName]>>,
    modules: Vec<ModuleExportSpec>,
    /// The polyfill's module slot for each static module the
    /// translator numbered.
    module_index_for_static: &'a HashMap<StaticModuleIndex, usize>,
}

/// Recursively project one component-level export into the
/// [`ExportTree`]. A lifted function lands as one [`ExportSpec`]
/// carrying the `path` of the instance exports that enclose it; a
/// static core module lands as one [`ModuleExportSpec`]; an
/// instance-typed export records its own path and recurses into its
/// items with that path threaded through. Any name is accepted for an
/// instance export, a WIT interface identifier or a plain label, as
/// Wasmtime accepts it.
#[allow(clippy::too_many_arguments)]
fn collect_export_spec(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
    state: &mut ProjectionState,
    trampolines: &TrampolineSlots,
    out: &mut ExportTree<'_>,
    path: &[ExternalName],
    name: &str,
    export_index: ExportIndex,
) -> Result<()> {
    let export = translation
        .component
        .export_items
        .get(export_index)
        .ok_or_else(|| Error::internal("export index from translator missing from export_items"))?;
    match export {
        EnvironExport::LiftedFunction { ty, func, options } => {
            let source = state.lift_core_def(func, trampolines)?;
            let canon = translation
                .component
                .options
                .get(*options)
                .ok_or_else(|| Error::internal("export OptionsIndex out of bounds"))?;
            let options = lift_canon_options(canon)?;
            out.functions.push(ExportSpec {
                name: name.to_owned(),
                path: path.into(),
                source,
                signature: Arc::new(Signature::new(projector.function(*ty)?)),
                options: Arc::new(options),
            });
            Ok(())
        }
        EnvironExport::ModuleStatic { index, .. } => {
            let module_index = *out.module_index_for_static.get(index).ok_or_else(|| {
                Error::internal("module export names a static module the translator did not emit")
            })?;
            out.modules.push(ModuleExportSpec {
                name: name.to_owned(),
                path: path.into(),
                source: ModuleSource::Static(module_index),
            });
            Ok(())
        }
        EnvironExport::ModuleImport { import, .. } => {
            let (import_index, item_path) = import_path(translation, *import)?;
            out.modules.push(ModuleExportSpec {
                name: name.to_owned(),
                path: path.into(),
                source: ModuleSource::Import {
                    import_index,
                    path: item_path,
                },
            });
            Ok(())
        }
        EnvironExport::Instance { exports, .. } => {
            let mut nested = path.to_vec();
            nested.push(ExternalName::from_raw(name));
            out.instances.push(nested.clone().into_boxed_slice());
            for (item_name, (inner_index, _)) in exports.raw_iter() {
                collect_export_spec(
                    translation,
                    projector,
                    state,
                    trampolines,
                    out,
                    &nested,
                    item_name,
                    *inner_index,
                )?;
            }
            Ok(())
        }
        EnvironExport::Type(_) => {
            // Type exports carry no runtime presence.
            Ok(())
        }
    }
}

/// Resolve a [`TypeResourceTableIndex`] to the polyfill's table
/// index: the same number, checked to name a concrete table.
fn resolve_table_index(
    component_types: &ComponentTypes,
    num_tables: usize,
    ty: TypeResourceTableIndex,
) -> Result<usize> {
    if let TypeResourceTable::Abstract(_) = &component_types[ty] {
        return Err(Error::internal(
            "resource trampoline references an abstract resource table in a concrete instantiation",
        ));
    }
    let index = ty.as_u32() as usize;
    if index < num_tables {
        Ok(index)
    } else {
        Err(Error::internal(
            "resource trampoline references a table outside the component's resource tables",
        ))
    }
}

/// The `count` stream or future tables of a component, each built by
/// `table` from its index.
fn end_tables(
    count: usize,
    table: impl Fn(u32) -> Result<EndTableSpec>,
) -> Result<Arc<[EndTableSpec]>> {
    (0..count as u32).map(table).collect()
}

/// The payload type of the stream or future type `ty`, which a
/// built-in that works on the type's ends was declared with, or
/// `None` for one that carries no values.
fn payload_of(projector: &TypeProjector<'_>, ty: InterfaceType) -> Result<Option<ValueType>> {
    match projector.value_type(&ty)? {
        ValueType::Stream(stream) => Ok(stream.payload().cloned()),
        ValueType::Future(future) => Ok(future.payload().cloned()),
        _ => Err(Error::internal(
            "a stream or future built-in names a type that is neither",
        )),
    }
}

/// The copy built-in on an end of `kind`, declared with `options` for
/// a stream or future of `payload`, with the path its copies take
/// selected from the payload.
fn copy_spec(
    kind: EndKind,
    options: CanonOptions,
    payload: Option<ValueType>,
    signature: CoreSignature,
) -> TrampolineSpec {
    TrampolineSpec::Copy {
        kind,
        options,
        copies_bytes: copies_bytes(payload.as_ref()),
        payload,
        signature,
    }
}

/// Whether a copy of `payload` moves bytes rather than values: the
/// payload is absent, or a number type, every bit pattern of which is
/// a valid value, so a byte copy and a value copy give the same bytes.
/// `bool` and `char` are left out because not every bit pattern is
/// valid for them. That is the set Wasmtime's compiler copies in one
/// step, and the set the reference lets one instance both read and
/// write.
fn copies_bytes(payload: Option<&ValueType>) -> bool {
    matches!(
        payload,
        None | Some(ValueType::Primitive(
            PrimitiveType::S8
                | PrimitiveType::U8
                | PrimitiveType::S16
                | PrimitiveType::U16
                | PrimitiveType::S32
                | PrimitiveType::U32
                | PrimitiveType::S64
                | PrimitiveType::U64
                | PrimitiveType::F32
                | PrimitiveType::F64
        ))
    )
}

/// Resolve a runtime import to the polyfill import index and the
/// item path inside it: one name per nesting level from the imported
/// instance down to the item, or empty when the import is the item.
/// The polyfill's import list is the translator's `import_types` in
/// order, so the import index is the translator's.
fn import_path(
    translation: &ComponentTranslation,
    runtime_import: RuntimeImportIndex,
) -> Result<(usize, Box<[String]>)> {
    let (import_idx, path) = translation
        .component
        .imports
        .get(runtime_import)
        .ok_or_else(|| Error::internal("RuntimeImportIndex out of bounds"))?;
    Ok((
        import_idx.as_u32() as usize,
        path.clone().into_boxed_slice(),
    ))
}

/// Working state for the main initializer walk. Carries only the
/// data the main walk produces; trampoline/lowering tables are
/// pre-built and read-only at this stage.
struct ProjectionState {
    runtime_instance_to_module: Vec<Option<usize>>,
    initializers: Vec<Initializer>,
    num_runtime_memories: usize,
    num_runtime_reallocs: usize,
    num_runtime_post_returns: usize,
    num_runtime_callbacks: usize,
    num_runtime_tables: usize,
    /// Trampoline specs created on demand for intrinsics that appear
    /// as `CoreDef`s. Their indices start at `spec_base`.
    extra_specs: Vec<TrampolineSpec>,
    spec_base: usize,
    intrinsic_to_spec: HashMap<UnsafeIntrinsic, usize>,
}

impl ProjectionState {
    fn new(spec_base: usize) -> Self {
        Self {
            runtime_instance_to_module: Vec::new(),
            initializers: Vec::new(),
            num_runtime_memories: 0,
            num_runtime_reallocs: 0,
            num_runtime_post_returns: 0,
            num_runtime_callbacks: 0,
            num_runtime_tables: 0,
            extra_specs: Vec::new(),
            spec_base,
            intrinsic_to_spec: HashMap::new(),
        }
    }

    /// The trampoline index of the spec for `intrinsic`, creating
    /// it on first use.
    fn intrinsic(&mut self, intrinsic: UnsafeIntrinsic) -> Result<usize> {
        if let Some(index) = self.intrinsic_to_spec.get(&intrinsic) {
            return Ok(*index);
        }
        let signature = CoreSignature {
            params: intrinsic
                .core_params()
                .iter()
                .map(lift_core_parameter)
                .collect::<Result<Vec<_>>>()?,
            results: intrinsic
                .core_results()
                .iter()
                .map(lift_core_val_type)
                .collect::<Result<Vec<_>>>()?,
        };
        let spec = match intrinsic {
            UnsafeIntrinsic::ContextGetI32_0 => TrampolineSpec::ContextGet { slot: 0, signature },
            UnsafeIntrinsic::ContextGetI32_1 => TrampolineSpec::ContextGet { slot: 1, signature },
            UnsafeIntrinsic::ContextSetI32_0 => TrampolineSpec::ContextSet { slot: 0, signature },
            UnsafeIntrinsic::ContextSetI32_1 => TrampolineSpec::ContextSet { slot: 1, signature },
            other => {
                return Err(Error::unsupported(format!(
                    "the `{}` intrinsic",
                    other.name()
                )));
            }
        };
        let index = self.spec_base + self.extra_specs.len();
        self.extra_specs.push(spec);
        self.intrinsic_to_spec.insert(intrinsic, index);
        Ok(index)
    }

    fn lift_core_def(
        &mut self,
        def: &CoreDef,
        trampolines: &TrampolineSlots,
    ) -> Result<ImportSource> {
        match def {
            CoreDef::Export(export) => self.lift_core_export(export),
            CoreDef::Trampoline(trampoline_idx) => match trampolines.get(trampoline_idx) {
                Some(slot) => Ok(ImportSource::Trampoline(*slot)),
                None => Err(Error::internal(
                    "a core definition names a trampoline the pre-walk did not see",
                )),
            },
            CoreDef::InstanceFlags(instance) => {
                Ok(ImportSource::InstanceFlags(instance.as_u32() as usize))
            }
            CoreDef::UnsafeIntrinsic(intrinsic) => {
                Ok(ImportSource::Trampoline(self.intrinsic(*intrinsic)?))
            }
        }
    }

    fn lift_core_export<U: Copy + Into<EnvironEntityIndex>>(
        &self,
        export: &CoreExport<U>,
    ) -> Result<ImportSource> {
        let _ = self
            .runtime_instance_to_module
            .get(export.instance.as_u32() as usize)
            .ok_or_else(|| {
                Error::internal("CoreExport instance index missing from projection map")
            })?;
        let item = match &export.item {
            EnvironExportItem::Name(s) => CoreSourceItem::Name(s.clone()),
            EnvironExportItem::Index(idx) => {
                CoreSourceItem::Index(lift_entity_index((*idx).into()))
            }
        };
        Ok(ImportSource::CoreInstanceExport(CoreInstanceExport {
            instance_index: export.instance.as_u32() as usize,
            item,
        }))
    }
}

/// The canon options a trampoline declared, by the index the
/// translator recorded for it.
fn trampoline_options(
    translation: &ComponentTranslation,
    options: OptionsIndex,
) -> Result<CanonOptions> {
    let canon = translation
        .component
        .options
        .get(options)
        .ok_or_else(|| Error::internal("Trampoline OptionsIndex out of bounds"))?;
    lift_canon_options(canon)
}

/// The `cancellable` immediate of the built-in whose canon options
/// `options` names, which is where Wasmtime 49 carries the immediate
/// of `waitable-set.wait` and `waitable-set.poll`.
fn trampoline_cancellable(
    translation: &ComponentTranslation,
    options: OptionsIndex,
) -> Result<bool> {
    translation
        .component
        .options
        .get(options)
        .map(|canon| canon.cancellable)
        .ok_or_else(|| Error::internal("Trampoline OptionsIndex out of bounds"))
}

/// Project one canon-options bundle onto the polyfill's own.
///
/// The garbage-collected data model reaches the field faithfully and
/// is then refused: the polyfill implements the linear-memory ABI
/// strategy alone, and an option it does not implement fails here
/// rather than at the crossing that would have used it.
fn lift_canon_options(options: &EnvironCanonOptions) -> Result<CanonOptions> {
    let (realloc, data_model) = match options.data_model {
        CanonicalOptionsDataModel::LinearMemory(opts) => (
            opts.realloc.map(|i| i.as_u32() as usize),
            DataModel::LinearMemory,
        ),
        CanonicalOptionsDataModel::Gc {} => (None, DataModel::Gc),
    };
    if data_model != DataModel::LinearMemory {
        return Err(Error::unsupported(
            "the garbage-collected canonical-ABI data model",
        ));
    }
    Ok(CanonOptions {
        instance: options.instance.as_u32() as usize,
        memory: options.memory().map(|i| i.as_u32() as usize),
        realloc,
        post_return: options.post_return.map(|i| i.as_u32() as usize),
        async_: options.async_,
        callback: options.callback.map(|i| i.as_u32() as usize),
        string_encoding: lift_string_encoding(options.string_encoding),
        data_model,
    })
}

fn lift_string_encoding(encoding: EnvironStringEncoding) -> StringEncoding {
    match encoding {
        EnvironStringEncoding::Utf8 => StringEncoding::Utf8,
        EnvironStringEncoding::Utf16 => StringEncoding::Utf16,
        EnvironStringEncoding::CompactUtf16 => StringEncoding::CompactUtf16,
    }
}

fn lift_entity_index(idx: EnvironEntityIndex) -> EntityIndex {
    match idx {
        EnvironEntityIndex::Function(i) => EntityIndex::Function(i.as_u32()),
        EnvironEntityIndex::Table(i) => EntityIndex::Table(i.as_u32()),
        EnvironEntityIndex::Memory(i) => EntityIndex::Memory(i.as_u32()),
        EnvironEntityIndex::Global(i) => EntityIndex::Global(i.as_u32()),
        EnvironEntityIndex::Tag(i) => EntityIndex::Tag(i.as_u32()),
    }
}

/// The validator's refusals of a feature whose engine gate is off,
/// each with the feature it is surfaced as. A gate the host did not
/// turn on is a feature the polyfill leaves out, not a malformed
/// binary, so each refusal is surfaced as [`Error::Unsupported`].
///
/// The stackful gate covers the stackful lift and nothing else, so
/// its refusal names that lift. The threading gate covers every
/// thread built-in, and the error-context gate every error-context
/// built-in and the `error-context` type. The validator words each
/// of those refusals the same way for every item, naming it; such a
/// refusal keeps the validator's message whole inside the feature,
/// so a reader matching the validator's text still finds it.
const GATE_REFUSALS: &[(&str, Option<&str>)] = &[
    (
        "requires the component model async stackful feature",
        Some(
            "stackful asynchronous lifts (`canon lift async` without a callback) \
             while `wasm_component_model_async_stackful` is off",
        ),
    ),
    ("requires the component model threading feature", None),
    ("requires the component model error-context feature", None),
];

/// Map a refusal of the translator, which validated `bytes` with
/// `features` over a backend that declares `capabilities`. Where the
/// component would validate with a capability the backend lacks, the
/// refusal is [`Error::Unsupported`] with the name of that capability;
/// otherwise it is the translator's own failure.
fn refusal(
    err: TranslatorError,
    bytes: &[u8],
    features: WasmFeatures,
    capabilities: Capabilities,
) -> Error {
    match missing_capability(bytes, features, capabilities) {
        Some(capability) => Error::unsupported(capability.name()),
        None => translation_error(err),
    }
}

/// Map a translator failure onto the polyfill's error model. A
/// validation failure keeps the byte offset the translator reports;
/// a feature the translator itself does not support, and a feature
/// behind an engine gate the host left off, are surfaced as
/// [`Error::Unsupported`].
fn translation_error(err: TranslatorError) -> Error {
    let error = match err.downcast::<WasmError>() {
        Ok(WasmError::InvalidWebAssembly { message, offset }) => {
            Error::InvalidComponentBinary { message, offset }
        }
        Ok(WasmError::Unsupported(feature)) => Error::unsupported(feature),
        Ok(other) => Error::InvalidComponentBinary {
            message: format!("{other}"),
            offset: 0,
        },
        Err(other) => Error::InvalidComponentBinary {
            message: format!("{other:#}"),
            offset: 0,
        },
    };
    let Error::InvalidComponentBinary { message, .. } = &error else {
        return error;
    };
    match GATE_REFUSALS
        .iter()
        .find(|(words, _)| message.contains(words))
    {
        Some((_, Some(feature))) => Error::unsupported(*feature),
        Some((_, None)) => Error::unsupported(message.clone()),
        None => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ListType, RecordField, RecordType};

    #[wcmp_macros::test]
    fn it_selects_the_byte_path_for_the_number_types_and_no_payload_alone() {
        use PrimitiveType::*;
        for number in [S8, U8, S16, U16, S32, U32, S64, U64, F32, F64] {
            assert!(
                copies_bytes(Some(&ValueType::Primitive(number))),
                "every bit pattern of {number:?} is a valid value"
            );
        }
        assert!(
            copies_bytes(None),
            "a payload that is absent moves no bytes at all"
        );
        for other in [
            ValueType::Primitive(Bool),
            ValueType::Primitive(Char),
            ValueType::Primitive(String),
            ValueType::List(ListType::new(ValueType::Primitive(U8))),
            ValueType::Record(RecordType::new([RecordField::new(
                "a",
                ValueType::Primitive(U32),
            )])),
        ] {
            assert!(
                !copies_bytes(Some(&other)),
                "{other:?} keeps the path through values"
            );
        }
    }
}
