//! An instantiated core module the host drove itself.

use crate::internal::{CoreExternInternal, CoreInstanceParts};
use crate::store::Store;
use crate::store::StoreInternalExt;

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
    /// The exports of the instance, by name, in the order the module
    /// declares them. The exports of an instance never change, so the
    /// instance reads them once, when it is made.
    exports: Vec<(String, CoreExtern)>,
}

impl From<CoreInstanceParts> for CoreInstance {
    fn from(parts: CoreInstanceParts) -> Self {
        Self {
            exports: parts.exports,
        }
    }
}

impl CoreInstance {
    /// Look up an export by the name the module declares it under.
    /// Returns `None` when the module exports nothing by that name,
    /// and when `store` is not the store the instance lives in.
    pub fn get_export<T: 'static>(&self, store: &Store<T>, name: &str) -> Option<CoreExtern> {
        self.exports
            .iter()
            .find(|(export, item)| export == name && item.store_id() == store.internal_ref().id())
            .map(|(_, item)| item.clone())
    }
}

impl core::fmt::Debug for CoreInstance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CoreInstance").finish_non_exhaustive()
    }
}
