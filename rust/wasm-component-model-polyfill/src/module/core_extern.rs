//! A core item the host holds: an export of a [`CoreInstance`].
//!
//! [`CoreInstance`]: super::CoreInstance

use wasm_runtime_layer::Extern as RuntimeExtern;

use crate::internal::{CoreExternInternal, CoreExternParts, CoreExternTypeInternal};
use crate::store::StoreInternalExt;
use crate::store::{Store, StoreId};

use super::core_extern_type::CoreExternType;

/// A core function, global, memory, or table the host holds.
///
/// A `CoreExtern` comes from [`CoreInstance::get_export`] and goes
/// back into [`Module::instantiate`] as an import of another core
/// module. The host inspects its type through [`Self::ty`] and
/// otherwise treats it as opaque: the component model moves values
/// across the boundary through lifted functions, not through core
/// items.
///
/// The value lives in the [`Store`] its instance was created in and
/// is refused as an import into a module instantiated through any
/// other store.
///
/// [`CoreInstance::get_export`]: super::CoreInstance::get_export
/// [`Module::instantiate`]: super::Module::instantiate
#[derive(Clone)]
pub struct CoreExtern {
    /// The runtime-layer item.
    inner: RuntimeExtern,
    /// The identity of the store the item lives in.
    store_id: StoreId,
}

impl From<CoreExternParts> for CoreExtern {
    fn from(parts: CoreExternParts) -> Self {
        Self {
            inner: parts.inner,
            store_id: parts.store_id,
        }
    }
}

impl CoreExternInternal for CoreExtern {
    fn inner(&self) -> &RuntimeExtern {
        &self.inner
    }

    fn store_id(&self) -> StoreId {
        self.store_id
    }
}

impl CoreExtern {
    /// The type of the item.
    pub fn ty<T: 'static>(&self, store: &Store<T>) -> CoreExternType {
        CoreExternType::from_runtime(&self.inner.ty(store.internal_ref().inner()))
    }
}

impl core::fmt::Debug for CoreExtern {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let kind = match &self.inner {
            RuntimeExtern::Func(_) => "func",
            RuntimeExtern::Global(_) => "global",
            RuntimeExtern::Memory(_) => "memory",
            RuntimeExtern::Table(_) => "table",
        };
        f.debug_struct("CoreExtern").field("kind", &kind).finish()
    }
}
