//! The polyfill's instantiated-component value.

use std::sync::{Arc, Mutex};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::signature::Signature;
use crate::component::ExternalName;
use crate::executor::ir::CanonOptions;
use crate::internal::{FuncParts, InstanceExportsInternal, InstanceInternal, InstanceParts};
use crate::module::Module;
use crate::store::StoreId;

use super::exports::InstanceExports;
use super::func::Func;

/// One exported function of an instantiated component, paired with
/// the component-level signature the polyfill's parsed-component
/// value declared for it. The signature is what [`Func::call`]
/// consults when lowering arguments and lifting results across the
/// canonical-ABI boundary.
///
/// Every [`Func`] handle for the export shares this record, so
/// looking the export up and calling it copies none of it.
///
/// Workspace-internal; never re-exported through `lib.rs`.
///
/// [`Func::call`]: crate::Func::call
pub struct ExportedFunction {
    /// The export's leaf name. For root-level exports this is the
    /// name the component publishes; for instance-typed exports
    /// this is the item name inside the enclosing instance.
    pub name: String,
    /// The names of the instance-typed exports that enclose this
    /// function, from the root of the export tree inward, or empty
    /// for a root-level function export. Used by the
    /// [`InstanceExports`] navigator to scope `func` lookups to the
    /// addressed instance.
    pub path: Box<[ExternalName]>,
    /// The runtime-layer core-Wasm function that backs this export.
    pub func: wasm_runtime_layer::Func,
    /// The polyfill's component-level signature for this export,
    /// with its canonical-ABI layout. Shared with the translation.
    pub signature: Arc<Signature>,
    /// The canonical-ABI options the export's lift declared. Used
    /// by [`Func::call`] to look up memory/realloc/post-return at
    /// call time.
    ///
    /// [`Func::call`]: crate::Func::call
    pub options: Arc<CanonOptions>,
}

/// One exported core module of an instantiated component.
///
/// Workspace-internal; never re-exported through `lib.rs`.
pub struct ExportedModule {
    /// The export's leaf name.
    pub name: String,
    /// The names of the instance-typed exports that enclose this
    /// module, from the root of the export tree inward, or empty for
    /// a root-level module export.
    pub path: Box<[ExternalName]>,
    /// The compiled module the export hands to the host.
    pub module: Module,
}

/// A successfully linked, instantiated component.
///
/// `Instance` is produced by
/// [`Linker::instantiate`](crate::Linker::instantiate); its lifetime
/// is logically bound to the [`Store`] it was created in. Multiple
/// instances of the same component can coexist in a single store
/// and remain isolated — each carries its own substrate-level
/// instance state. The store is the unit of isolation.
///
/// The substrate state is a set of one or more runtime-layer
/// core-Wasm instances (one per `(core module ...)` section of the
/// component, instantiated in component-section order with their
/// imports stitched up by the polyfill's component executor); the
/// fields are workspace-internal and never reach the public API.
pub struct Instance {
    /// The runtime-layer core-Wasm instances that back this
    /// component instance, indexed in component-section order. Held
    /// for as long as the component instance lives, which is what
    /// keeps the guest's core state alive; nothing reads it.
    #[allow(dead_code)]
    core_instances: Box<[wasm_runtime_layer::Instance]>,
    /// The component-level function exports the executor produced
    /// when wiring the component.
    function_exports: Box<[Arc<ExportedFunction>]>,
    /// The path of every instance-typed export, at any depth, in
    /// declaration order. An instance export is listed whether or
    /// not it holds a function, so the navigator can reach an empty
    /// instance.
    instance_exports: Box<[Box<[ExternalName]>]>,
    /// The module-typed exports, at any depth, in declaration order.
    module_exports: Box<[ExportedModule]>,
    /// The canonical-ABI runtime state populated during
    /// instantiation: the per-component slabs of memories,
    /// reallocs, and post-returns. Held inside an `Arc<Mutex<…>>`
    /// because trampolines built for `LowerImport` directives
    /// share access to the same slabs at call time, and the
    /// runtime layer's `Func::new` requires `Send + Sync`
    /// closures.
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The identity of the [`Store`] this instance was created in.
    /// Every [`Func`] handed out by this instance carries it, so a
    /// call through another store is rejected before it reaches
    /// the runtime layer.
    ///
    /// [`Store`]: crate::Store
    store_id: StoreId,
}

impl From<InstanceParts> for Instance {
    fn from(parts: InstanceParts) -> Self {
        Self {
            core_instances: parts.core_instances,
            function_exports: parts.function_exports,
            instance_exports: parts.instance_exports,
            module_exports: parts.module_exports,
            abi_state: parts.abi_state,
            store_id: parts.store_id,
        }
    }
}

impl InstanceInternal for Instance {
    fn function_export(&self, path: &[ExternalName], name: &str) -> Option<Func> {
        self.function_exports
            .iter()
            .find(|export| export.path.as_ref() == path && export.name == name)
            .map(|export| self.func_for(export))
    }

    fn module_export(&self, path: &[ExternalName], name: &str) -> Option<Module> {
        self.module_exports
            .iter()
            .find(|export| export.path.as_ref() == path && export.name == name)
            .map(|export| export.module.clone())
    }

    fn has_instance_export(&self, path: &[ExternalName]) -> bool {
        self.instance_exports
            .iter()
            .any(|export| export.as_ref() == path)
    }

    fn func_for(&self, export: &Arc<ExportedFunction>) -> Func {
        FuncParts {
            export: Arc::clone(export),
            abi_state: self.abi_state.clone(),
            store_id: self.store_id,
        }
        .into()
    }
}

impl Instance {
    /// Look up a root-level exported function by its declared name.
    /// Returns `None` if the export is absent, not a function, or
    /// nested inside an instance-typed export. Use
    /// [`Self::exports`] to traverse instance-typed exports.
    pub fn get_func(&self, name: &str) -> Option<Func> {
        self.function_export(&[], name)
    }

    /// Look up a root-level exported core module by its declared
    /// name. Returns `None` if the export is absent, not a module, or
    /// nested inside an instance-typed export. Use [`Self::exports`]
    /// to traverse instance-typed exports.
    pub fn get_module(&self, name: &str) -> Option<Module> {
        self.module_export(&[], name)
    }

    /// The export navigator for this instance.
    ///
    /// The returned [`InstanceExports`] borrows from `self` and
    /// exposes both root-level function exports (via
    /// [`InstanceExports::func`]) and the per-interface lookup that
    /// reaches into instance-typed exports (via
    /// [`InstanceExports::instance`]). The navigator complements
    /// [`Self::get_func`]: the latter is the unchanged shorthand
    /// for root-level function exports, while the navigator adds
    /// the instance-typed traversal `get_func` cannot see.
    pub fn exports(&self) -> InstanceExports<'_> {
        InstanceExports::new(self)
    }
}
