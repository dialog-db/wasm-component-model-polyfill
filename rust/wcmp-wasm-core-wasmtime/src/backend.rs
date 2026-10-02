// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Wasmtime backend.

use core::fmt;
use std::sync::Arc;

use wcmp_wasm_core::backend::{
    Backend, BackendModule, BackendStore, BoxFuture, StoreData, missing_capability,
};
use wcmp_wasm_core::{Capabilities, Capability, Error, Result};

use crate::errors;
use crate::module::WasmtimeModule;
use crate::state::State;
use crate::store::WasmtimeStore;
use crate::type_registry::TypeRegistry;

/// The Wasmtime backend of the runtime layer: the control.
///
/// A host hands one to
/// [`Engine::with_backend`](wcmp_wasm_core::Engine::with_backend). Each
/// value holds its own Wasmtime engine, so two engines over two values of
/// this type share nothing.
pub struct Wasmtime {
    engine: wasmtime::Engine,
    capabilities: Capabilities,
    types: Arc<TypeRegistry>,
}

impl Wasmtime {
    /// The capabilities the backend declares on every platform.
    const FEATURES: [Capability; 8] = [
        Capability::MultiMemory,
        Capability::Memory64,
        Capability::TailCall,
        Capability::Exceptions,
        Capability::FunctionReferences,
        Capability::Gc,
        Capability::RelaxedSimd,
        Capability::Threads,
    ];

    /// A backend over a new Wasmtime engine, with every feature the backend
    /// declares turned on.
    ///
    /// Stack switching is on where Wasmtime implements it, and declared
    /// there. A Wasmtime that refuses the configuration, as it would on a
    /// platform its compiler does not serve, is [`Error::Backend`].
    pub fn new() -> Result<Self> {
        let mut config = wasmtime::Config::new();
        config
            .wasm_multi_memory(true)
            .wasm_memory64(true)
            .wasm_tail_call(true)
            .wasm_exceptions(true)
            .wasm_function_references(true)
            .wasm_gc(true)
            .wasm_relaxed_simd(true)
            .wasm_threads(true)
            .shared_memory(true);
        let capabilities = Capabilities::from_iter(Self::FEATURES);
        // Wasmtime's compiler serves stack switching only on some
        // platforms, and refuses an engine that asks for it anywhere else.
        config.wasm_stack_switching(true);
        let (engine, capabilities) = match wasmtime::Engine::new(&config) {
            Ok(engine) => (engine, capabilities.with(Capability::StackSwitching)),
            Err(_) => {
                config.wasm_stack_switching(false);
                let engine = wasmtime::Engine::new(&config).map_err(errors::backend)?;
                (engine, capabilities)
            }
        };
        Ok(Self {
            engine,
            capabilities,
            types: Arc::default(),
        })
    }
}

impl Backend for Wasmtime {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        // Wasmtime compiles synchronously, so the future is ready on its
        // first poll.
        Box::pin(core::future::ready(self.compile_sync(bytes)))
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        // `from_binary`, and not `new`, so that the text format is refused
        // here as every other engine refuses it.
        let module = wasmtime::Module::from_binary(&self.engine, bytes).map_err(|error| {
            match missing_capability(self.capabilities, bytes) {
                Some(capability) => Error::Unsupported(capability),
                None => Error::Compile {
                    message: format!("{error:#}"),
                },
            }
        })?;
        Ok(Box::new(WasmtimeModule::new(module, &self.types)))
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        let state = State::new(data, self.types.clone());
        Ok(Box::new(WasmtimeStore::new(wasmtime::Store::new(
            &self.engine,
            state,
        ))))
    }
}

impl fmt::Debug for Wasmtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wasmtime")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    /// The Wasmtime module that `module` holds.
    fn wasmtime_module(module: &dyn BackendModule) -> wasmtime::Module {
        module
            .as_any()
            .downcast_ref::<WasmtimeModule>()
            .expect("the backend compiles its own modules")
            .module()
            .clone()
    }

    #[wcmp_macros::test]
    fn it_compiles_a_module_of_its_own_each_time() {
        let backend = Wasmtime::new().expect("Wasmtime makes an engine");
        let bytes = wcmp_macros::wasm!(r#"(module (func (export "run")))"#);

        let first = backend.compile_sync(bytes).expect("the module compiles");
        let second = backend.compile_sync(bytes).expect("the module compiles");
        let Poll::Ready(third) =
            pin!(backend.compile(bytes)).poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("the compile is ready on its first poll");
        };
        let third = third.expect("the module compiles");

        let modules = [&first, &second, &third].map(|module| wasmtime_module(&**module));
        for (index, module) in modules.iter().enumerate() {
            for other in &modules[index + 1..] {
                assert!(
                    !wasmtime::Module::same(module, other),
                    "each compile makes a module of its own"
                );
            }
        }
    }
}
