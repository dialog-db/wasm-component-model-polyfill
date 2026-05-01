//! The polyfill's instantiated-component value.

use crate::component::FunctionType;
use crate::store::Store;

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
    /// The export's declared name.
    pub name: String,
    /// The runtime-layer core-Wasm function that backs this export.
    pub func: wasm_runtime_layer::Func,
    /// The polyfill's component-level signature for this export.
    pub signature: FunctionType,
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
}

impl Instance {
    /// Look up an exported function by its declared name. Returns
    /// `None` if the export is absent or is not a function.
    ///
    /// `T` is the host-data type of the [`Store`] the instance was
    /// created in. The store is taken so future work that needs to
    /// realise lift/lower context for compound valtypes can do so
    /// against the same store the instance lives in.
    pub fn get_func<T>(&self, _store: &mut Store<T>, name: &str) -> Option<Func> {
        self.function_exports
            .iter()
            .find(|export| export.name == name)
            .map(|export| Func {
                inner: export.func.clone(),
                signature: export.signature.clone(),
            })
    }
}
