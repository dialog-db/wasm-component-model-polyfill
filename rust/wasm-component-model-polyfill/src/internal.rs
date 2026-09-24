//! The crate-internal faces of the types `lib.rs` re-exports.
//!
//! A type `lib.rs` re-exports carries its whole `pub` surface into
//! the public API, whether or not the values that surface hands back
//! can be named outside the crate: a method is reachable by method
//! syntax, and a field by field syntax, however unnameable its type
//! is. Everything the crate needs of such a type and the public API
//! does not therefore lives here, on an extension trait this module
//! declares and the type's own module implements.
//!
//! This module is private and `lib.rs` re-exports nothing from it, so
//! a trait here cannot be imported outside the crate and none of its
//! entries resolve there. A `Parts` struct beside a trait is the same
//! rule for construction: the crate builds the value from it, and no
//! caller outside can name it.
//!
//! [`Store`] and [`StoreContext`] carry an internal surface too large
//! for a trait, so theirs is a wrapper struct over a borrow instead:
//! see [`StoreInternal`] and [`StoreContextInternal`].
//!
//! [`Store`]: crate::Store
//! [`StoreContext`]: crate::StoreContext
//! [`StoreInternal`]: crate::store::StoreInternal
//! [`StoreContextInternal`]: crate::store::StoreContextInternal

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{
    Extern as RuntimeExtern, ExternType as RuntimeExternType, Instance as RuntimeInstance,
    Module as RuntimeModule, RefType, ValType as RuntimeValType,
};
use wasmtime_environ::wasmparser::WasmFeatures;

use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::shape::AbiShape;
use crate::backend::Backend;
use crate::component::{Component, ExternalName, FunctionType};
use crate::concurrency::{CopyBuffer, EndId};
use crate::error::{Error, Result};
use crate::executor::ir::{CanonOptions, ExecutorIr};
use crate::identifier::InterfaceIdentifier;
use crate::instance::{ExportedFunction, ExportedModule, Func, Instance, TypedFunc};
use crate::linker::{DestructorBody, InstanceRegistration, Resolution};
use crate::module::{CoreExternType, CoreValueType, Module};
use crate::resource::ResourceTableRuntime;
use crate::store::{StoreContext, StoreId};
use crate::types::ValueType;

/// The crate-internal face of [`Component`](crate::Component).
pub trait ComponentInternal {
    /// The executor's plan for this component.
    fn ir(&self) -> &ExecutorIr;
}

/// The crate-internal face of [`Engine`](crate::Engine).
pub trait EngineInternal {
    /// Borrow the wrapped runtime-layer engine.
    fn inner(&self) -> &wasm_runtime_layer::Engine<Backend>;
}

/// The crate-internal face of [`EngineConfig`](crate::EngineConfig).
pub trait EngineConfigInternal {
    /// The validator features this configuration selects.
    fn wasm_features(&self) -> WasmFeatures;
}

/// The crate-internal face of a compound value type: a
/// [`RecordType`](crate::RecordType), [`TupleType`](crate::TupleType),
/// [`VariantType`](crate::VariantType), [`OptionType`](crate::OptionType),
/// [`ResultType`](crate::ResultType), or
/// [`FixedLengthListType`](crate::FixedLengthListType).
pub trait CompoundTypeInternal {
    /// The canonical-ABI shape the type computed when it was built.
    fn abi_shape(&self) -> &AbiShape;
}

/// The crate-internal face of [`Error`](crate::Error).
pub trait ErrorInternal {
    /// Build an `Error::Unsupported` naming `feature`.
    fn unsupported(feature: impl Into<String>) -> Error;

    /// Build an `Error::Internal` carrying `message`.
    fn internal(message: impl Into<String>) -> Error;
}

/// The crate-internal face of [`Accessor`](crate::Accessor).
pub trait AccessorInternal<T: 'static> {
    /// A token for the store `store` names.
    fn new(store: StoreId) -> Self;
}

/// The crate-internal face of [`Destination`](crate::Destination).
pub trait DestinationInternal<'a, T> {
    /// The destination of a read that can take `remaining` items,
    /// over the vector the end keeps its waiting items in.
    fn new(buffer: &'a mut Vec<T>, remaining: Option<usize>) -> Self;
}

/// The crate-internal face of [`Source`](crate::Source).
pub trait SourceInternal<'a, T> {
    /// The source of a write whose items a host producer delivered,
    /// taken from the front of `items`.
    fn host(items: &'a mut Vec<T>) -> Self;
    /// The source of a guest's write over `write`, its buffer as it
    /// stood when the poll began, counting the items the poll takes
    /// in `taken`.
    fn guest(write: &'a CopyBuffer, taken: &'a mut u32) -> Self;
}

/// The crate-internal face of [`StreamReader`](crate::StreamReader).
pub trait StreamReaderInternal {
    /// The reader of the readable end `end`.
    fn from_end(end: EndId) -> Self;
    /// The readable end the reader holds.
    fn end(&self) -> EndId;
}

/// The crate-internal face of [`FutureReader`](crate::FutureReader).
pub trait FutureReaderInternal {
    /// The reader of the readable end `end`.
    fn from_end(end: EndId) -> Self;
    /// The readable end the reader holds.
    fn end(&self) -> EndId;
}

/// The crate-internal face of [`StreamAny`](crate::StreamAny).
pub trait StreamAnyInternal {
    /// The untyped value of the readable end `end`, whose stream
    /// carries `payload`.
    fn new(end: EndId, payload: Option<ValueType>) -> Self;
    /// The readable end the value holds.
    fn end(&self) -> EndId;
    /// The type of the values the stream carries.
    fn payload(&self) -> Option<&ValueType>;
}

/// The crate-internal face of [`FutureAny`](crate::FutureAny).
pub trait FutureAnyInternal {
    /// The untyped value of the readable end `end`, whose future
    /// carries `payload`.
    fn new(end: EndId, payload: Option<ValueType>) -> Self;
    /// The readable end the value holds.
    fn end(&self) -> EndId;
    /// The type of the value the future carries.
    fn payload(&self) -> Option<&ValueType>;
}

/// The crate-internal face of
/// [`InstanceExports`](crate::InstanceExports).
pub trait InstanceExportsInternal<'a> {
    /// The navigator for an instance.
    fn new(instance: &'a Instance) -> Self;
}

/// The crate-internal face of
/// [`ExportInstance`](crate::ExportInstance).
pub trait ExportInstanceInternal<'a> {
    /// A view onto the instance-typed export at `path`.
    fn new(instance: &'a Instance, path: Box<[ExternalName]>) -> Self;
}

/// The crate-internal face of [`TypedFunc`](crate::TypedFunc).
pub trait TypedFuncInternal<P, R> {
    /// A typed handle over an untyped export whose signature the
    /// caller has already checked.
    fn from_checked(inner: Func) -> TypedFunc<P, R>;
}

/// The parts a [`Func`](crate::Func) is built from.
pub struct FuncParts {
    /// The export the handle calls: its name, its core function, its
    /// signature, and its canon options, shared with the instance.
    pub export: Arc<ExportedFunction>,
    /// The instance's canonical-ABI runtime state.
    pub abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The identity of the store the owning instance was created in.
    pub store_id: StoreId,
}

/// The crate-internal face of [`Func`](crate::Func).
pub trait FuncInternal {
    /// The export's leaf name.
    fn name(&self) -> &str;

    /// The component-level signature of the export.
    fn signature(&self) -> &FunctionType;

    /// The canonical-ABI options the export's lift declared.
    fn options(&self) -> &CanonOptions;

    /// The instance's canonical-ABI runtime state.
    fn abi_state(&self) -> &Arc<Mutex<AbiRuntimeState>>;
}

/// The parts an [`Instance`](crate::Instance) is built from.
pub struct InstanceParts {
    /// The runtime-layer core instances that back the component
    /// instance, in component-section order.
    pub core_instances: Box<[RuntimeInstance]>,
    /// The component-level function exports.
    pub function_exports: Box<[Arc<ExportedFunction>]>,
    /// The path of every instance-typed export, at any depth.
    pub instance_exports: Box<[Box<[ExternalName]>]>,
    /// The module-typed exports, at any depth.
    pub module_exports: Box<[ExportedModule]>,
    /// The canonical-ABI runtime state instantiation populated.
    pub abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The identity of the store the instance was created in.
    pub store_id: StoreId,
}

/// The crate-internal face of [`Instance`](crate::Instance).
pub trait InstanceInternal {
    /// The function export named `name` inside the instance-typed
    /// export at `path`, or at the root when `path` is empty.
    fn function_export(&self, path: &[ExternalName], name: &str) -> Option<Func>;

    /// The module export named `name` inside the instance-typed
    /// export at `path`, or at the root when `path` is empty.
    fn module_export(&self, path: &[ExternalName], name: &str) -> Option<Module>;

    /// Whether the instance publishes an instance-typed export at
    /// `path`.
    fn has_instance_export(&self, path: &[ExternalName]) -> bool;

    /// The public handle for one of this instance's exports.
    fn func_for(&self, export: &Arc<ExportedFunction>) -> Func;
}

/// The crate-internal face of [`HostCall`](crate::HostCall).
pub trait HostCallInternal<'a, T: 'static> {
    /// The context for one call of a host function.
    fn new(
        store: StoreContext<'a, T>,
        resource_tables: Arc<[Option<ResourceTableRuntime>]>,
    ) -> Self;
}

/// The crate-internal face of
/// [`HostResource`](crate::HostResource).
pub trait HostResourceInternal<T> {
    /// The destructor the guest drop runs against.
    fn destructor(&self) -> &Arc<DestructorBody<T>>;
}

/// The crate-internal face of
/// [`LinkerInstance`](crate::LinkerInstance).
pub trait LinkerInstanceInternal<'a, T: 'static> {
    /// A borrowed view onto the given registration, which refuses a
    /// registration under a name the entry already holds an item
    /// under unless `allow_shadowing`.
    fn new(registration: &'a mut InstanceRegistration<T>, allow_shadowing: bool) -> Self;
}

/// The crate-internal face of [`Linker`](crate::Linker).
pub trait LinkerInternal<T: 'static> {
    /// The registered linker-instance keys, in insertion order.
    fn registered_keys(&self) -> impl Iterator<Item = &InterfaceIdentifier>;

    /// The registration entry for `id`, when one exists.
    fn registration_for(&self, id: &InterfaceIdentifier) -> Option<&InstanceRegistration<T>>;

    /// The root namespace's registration entry.
    fn root_registration(&self) -> &InstanceRegistration<T>;

    /// Drive the runtime substrate against an already-resolved
    /// import binding.
    fn instantiate_resolved(
        &self,
        store: &mut StoreContext<'_, T>,
        component: &Component,
        resolution: &Resolution,
    ) -> Result<Instance>;
}

/// The crate-internal face of [`Module`](crate::Module).
pub trait ModuleInternal {
    /// Wrap a runtime-layer module already compiled from `bytes`,
    /// reading its imports and exports from the binary.
    fn from_compiled(inner: RuntimeModule, bytes: &[u8]) -> Result<Module>;

    /// Borrow the wrapped runtime-layer module.
    fn inner(&self) -> &RuntimeModule;
}

/// The parts a [`CoreInstance`](crate::CoreInstance) is built from.
pub struct CoreInstanceParts {
    /// The runtime-layer instance.
    pub inner: RuntimeInstance,
    /// The identity of the store the instance lives in.
    pub store_id: StoreId,
}

/// The parts a [`CoreExtern`](crate::CoreExtern) is built from.
pub struct CoreExternParts {
    /// The runtime-layer item.
    pub inner: RuntimeExtern,
    /// The identity of the store the item lives in.
    pub store_id: StoreId,
}

/// The crate-internal face of [`CoreExtern`](crate::CoreExtern).
pub trait CoreExternInternal {
    /// Borrow the wrapped runtime-layer item.
    fn inner(&self) -> &RuntimeExtern;

    /// The identity of the store the item lives in.
    fn store_id(&self) -> StoreId;
}

/// The crate-internal face of
/// [`CoreExternType`](crate::CoreExternType).
pub trait CoreExternTypeInternal {
    /// Project a runtime-layer extern type.
    fn from_runtime(ty: &RuntimeExternType) -> CoreExternType;
}

/// The crate-internal face of
/// [`CoreValueType`](crate::CoreValueType).
pub trait CoreValueTypeInternal {
    /// Project a runtime-layer value type.
    fn from_runtime(ty: RuntimeValType) -> CoreValueType;

    /// Project a runtime-layer reference type.
    fn from_runtime_ref(ty: RefType) -> CoreValueType;

    /// Project a translator value type.
    fn from_translator(ty: &wasmtime_environ::WasmValType) -> Result<CoreValueType>;
}

/// The crate-internal face of
/// [`ResourceTypeId`](crate::ResourceTypeId).
pub trait ResourceTypeIdInternal {
    /// Mint a globally unique resource-type identity.
    fn fresh() -> Self;
}
