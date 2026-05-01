//! The polyfill's instantiated-component value.

use std::sync::{Arc, Mutex};

use crate::component::FunctionType;
use crate::executor::ir::CanonOptions;
use crate::executor::trampoline::AbiRuntimeState;
use crate::identifier::InterfaceIdentifier;
use crate::store::Store;

use super::exports::InstanceExports;
use super::func::Func;

/// One exported function of an instantiated component, paired with
/// the component-level [`FunctionType`] the polyfill's parsed-
/// component value declared for it. The signature is what
/// [`Func::call`] consults when lowering arguments and lifting
/// results across the canonical-ABI boundary.
///
/// Workspace-internal; never re-exported through `lib.rs`.
///
/// [`Func::call`]: crate::Func::call
pub struct ExportedFunction {
    /// The export's leaf name. For root-level exports this is the
    /// name the component publishes; for instance-typed exports
    /// this is the item name inside the enclosing instance.
    pub name: String,
    /// The enclosing instance-typed export's identifier when this
    /// function is nested inside one, or `None` for a root-level
    /// function export. Used by the [`InstanceExports`] navigator
    /// to scope `func` lookups to the addressed instance.
    pub parent: Option<InterfaceIdentifier>,
    /// The runtime-layer core-Wasm function that backs this export.
    pub func: wasm_runtime_layer::Func,
    /// The polyfill's component-level signature for this export.
    pub signature: FunctionType,
    /// The canonical-ABI options the export's lift declared. Used
    /// by [`Func::call`] to look up memory/realloc/post-return at
    /// call time.
    ///
    /// [`Func::call`]: crate::Func::call
    pub options: CanonOptions,
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
    /// component instance, indexed in component-section order.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub core_instances: Box<[wasm_runtime_layer::Instance]>,
    /// The component-level function exports the executor produced
    /// when wiring the component. Workspace-internal; never
    /// re-exported through `lib.rs`.
    pub function_exports: Box<[ExportedFunction]>,
    /// The canonical-ABI runtime state populated during
    /// instantiation: the per-component slabs of memories,
    /// reallocs, and post-returns. Held inside an `Arc<Mutex<…>>`
    /// because trampolines built for `LowerImport` directives
    /// share access to the same slabs at call time, and the
    /// runtime layer's `Func::new` requires `Send + Sync`
    /// closures.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub abi_state: Arc<Mutex<AbiRuntimeState>>,
}

impl Instance {
    /// Look up a root-level exported function by its declared name.
    /// Returns `None` if the export is absent, not a function, or
    /// nested inside an instance-typed export. Use
    /// [`Self::exports`] to traverse instance-typed exports.
    ///
    /// `T` is the host-data type of the [`Store`] the instance was
    /// created in. The store is taken so future work that needs to
    /// realise lift/lower context for compound valtypes can do so
    /// against the same store the instance lives in.
    pub fn get_func<T>(&self, _store: &mut Store<T>, name: &str) -> Option<Func> {
        self.function_exports
            .iter()
            .find(|export| export.parent.is_none() && export.name == name)
            .map(|export| Func {
                name: export.name.clone(),
                inner: export.func.clone(),
                signature: export.signature.clone(),
                options: export.options.clone(),
                abi_state: self.abi_state.clone(),
            })
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
