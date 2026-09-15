//! An instantiated core module the host drove itself.

use wasm_runtime_layer::Instance as RuntimeInstance;

use crate::store::{Store, StoreId};

use super::core_extern::CoreExtern;

/// A core module instantiated by the host through
/// [`Module::instantiate`].
///
/// The instance lives in the [`Store`] it was created in. Its exports
/// are reached by name as [`CoreExtern`] values, which the host can
/// feed into the instantiation of another core module. The polyfill's
/// component-level [`Instance`] is a different type: it is produced
/// by linking a component, and its exports are lifted functions.
///
/// [`Module::instantiate`]: super::Module::instantiate
/// [`Instance`]: crate::Instance
pub struct CoreInstance {
    /// The runtime-layer instance. Workspace-internal; never
    /// re-exported through `lib.rs`.
    pub inner: RuntimeInstance,
    /// The identity of the store the instance lives in.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub store_id: StoreId,
}

impl CoreInstance {
    /// Look up an export by the name the module declares it under.
    /// Returns `None` when the module exports nothing by that name.
    pub fn get_export<T: 'static>(&self, store: &Store<T>, name: &str) -> Option<CoreExtern> {
        self.inner
            .get_export(store.inner(), name)
            .map(|inner| CoreExtern {
                inner,
                store_id: self.store_id,
            })
    }
}

impl core::fmt::Debug for CoreInstance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CoreInstance").finish_non_exhaustive()
    }
}
