// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An instance of a module.

use crate::checks;
use crate::error::{Error, Result};
use crate::externs::Extern;
use crate::internal::{ModuleInternal, StoreContextMutInternal};
use crate::module::Module;
use crate::store::AsContextMut;

handle! {
    /// An instance of a module, in the store that instantiated it.
    Instance
}

impl Instance {
    /// Instantiates `module` in `store`, with `imports`: one extern for
    /// each import of the module, in order, as Wasmtime's `Instance::new`
    /// takes them. There is no linker.
    ///
    /// Every backend instantiates asynchronously. The browser backend uses
    /// `WebAssembly.instantiate`, so a module above the browser's limit for
    /// a synchronous instantiation loads too.
    ///
    /// The module must come from the engine of the store
    /// ([`Error::WrongEngine`]), each import must belong to the store
    /// ([`Error::WrongStore`]), and the number of imports must match the
    /// module ([`Error::ImportCount`]). An import of the wrong kind or type
    /// is [`Error::Link`], and a trap in the start function is
    /// [`Error::Trap`].
    pub async fn instantiate(
        mut store: impl AsContextMut,
        module: &Module,
        imports: &[Extern],
    ) -> Result<Self> {
        let mut store = store.as_context_mut();
        if !crate::Engine::same(store.engine(), module.engine()) {
            return Err(Error::WrongEngine);
        }
        let backend = store.backend_mut();
        imports
            .iter()
            .try_for_each(|import| checks::extern_in_store(backend, import))?;
        let expected = module.imports().len();
        if imports.len() != expected {
            return Err(Error::ImportCount {
                expected,
                actual: imports.len(),
            });
        }
        backend.instantiate(module.backend_module(), imports).await
    }

    /// The export of the instance named `name`, or `None` where the
    /// instance exports nothing by that name.
    pub fn get_export(&self, mut store: impl AsContextMut, name: &str) -> Result<Option<Extern>> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.instance_export(*self, name)
    }
}
