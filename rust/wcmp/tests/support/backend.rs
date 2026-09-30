//! The backend the polyfill's tests hand their engines.
//!
//! The polyfill has no backend of its own, so a test names one: the
//! browser's engine in the browser, and natively the backend that
//! `WCMP_TEST_BACKEND` names when the tests run, `wasmtime` or
//! `wasmi`, with Wasmtime when the variable is unset. The choice is
//! made at run time so that one native build runs on either backend:
//! the Wasmi lanes replay the archive the Wasmtime lanes built.
//!
//! The crate's unit tests reach this file through `src/runtime_layer.rs`
//! and its integration tests through a `#[path]` module of their own,
//! so each item is used by some of them and not by others.

#![allow(dead_code)]

#[cfg(not(target_arch = "wasm32"))]
use wcmp_wasm_core::backend::{Backend, BackendModule, BackendStore, BoxFuture, StoreData};
#[cfg(not(target_arch = "wasm32"))]
use wcmp_wasm_core::{Capabilities, Capability, Result};

/// The variable that names the native backend of a run.
#[cfg(not(target_arch = "wasm32"))]
pub const VARIABLE: &str = "WCMP_TEST_BACKEND";

/// A backend a native test can run on.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The Wasmtime backend, the control.
    Wasmtime,
    /// The Wasmi backend, which declares neither exception handling nor
    /// GC, and fills host suspension with Wasmi's resumable calls.
    Wasmi,
}

#[cfg(not(target_arch = "wasm32"))]
impl Kind {
    /// The backend `WCMP_TEST_BACKEND` names: Wasmtime when it is
    /// unset. A value that names no backend panics, which fails the
    /// test rather than running it on a backend the lane did not ask
    /// for.
    pub fn of_run() -> Self {
        match std::env::var(VARIABLE) {
            Err(std::env::VarError::NotPresent) => Kind::Wasmtime,
            Ok(name) if name == "wasmtime" => Kind::Wasmtime,
            Ok(name) if name == "wasmi" => Kind::Wasmi,
            Ok(name) => panic!("{VARIABLE}={name} names no backend (`wasmtime` or `wasmi`)"),
            Err(error) => panic!("{VARIABLE}: {error}"),
        }
    }

    /// The backend's name, as `WCMP_TEST_BACKEND` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Wasmtime => "wasmtime",
            Kind::Wasmi => "wasmi",
        }
    }

    /// A new backend of this kind.
    pub fn backend(self) -> TestBackend {
        match self {
            Kind::Wasmtime => TestBackend::Wasmtime(
                wcmp_wasm_core_wasmtime::Wasmtime::new().expect("Wasmtime makes an engine"),
            ),
            Kind::Wasmi => TestBackend::Wasmi(wcmp_wasm_core_wasmi::Wasmi::new()),
        }
    }
}

/// A native backend, of the kind the run chose. It forwards every
/// call to the backend it holds.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub enum TestBackend {
    /// The Wasmtime backend.
    Wasmtime(wcmp_wasm_core_wasmtime::Wasmtime),
    /// The Wasmi backend.
    Wasmi(wcmp_wasm_core_wasmi::Wasmi),
}

#[cfg(not(target_arch = "wasm32"))]
impl Backend for TestBackend {
    fn capabilities(&self) -> Capabilities {
        match self {
            TestBackend::Wasmtime(backend) => backend.capabilities(),
            TestBackend::Wasmi(backend) => backend.capabilities(),
        }
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        match self {
            TestBackend::Wasmtime(backend) => backend.compile(bytes),
            TestBackend::Wasmi(backend) => backend.compile(bytes),
        }
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        match self {
            TestBackend::Wasmtime(backend) => backend.compile_sync(bytes),
            TestBackend::Wasmi(backend) => backend.compile_sync(bytes),
        }
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        match self {
            TestBackend::Wasmtime(backend) => backend.new_store(data),
            TestBackend::Wasmi(backend) => backend.new_store(data),
        }
    }
}

/// The backend of this run: the one `WCMP_TEST_BACKEND` names.
#[cfg(not(target_arch = "wasm32"))]
pub fn backend() -> TestBackend {
    Kind::of_run().backend()
}

/// The backend of this run where it declares `capability`, and
/// Wasmtime, which declares it, where it does not. A test of a feature
/// that needs the capability runs the feature on every lane this way:
/// over Wasmi the component would stop at `Component::new` with
/// `Unsupported`, before the feature runs.
#[cfg(not(target_arch = "wasm32"))]
pub fn backend_declaring(capability: Capability) -> TestBackend {
    let backend = backend();
    if backend.capabilities().contains(capability) {
        backend
    } else {
        Kind::Wasmtime.backend()
    }
}

/// The name of the backend of this run: `wasmtime` or `wasmi`.
#[cfg(not(target_arch = "wasm32"))]
pub fn name() -> &'static str {
    Kind::of_run().name()
}

/// The browser backend of the runtime layer.
#[cfg(target_arch = "wasm32")]
pub fn backend() -> wcmp_wasm_core_web::Web {
    wcmp_wasm_core_web::Web::new()
}

/// The browser backend, whatever `capability` a test needs of it: the
/// browser is the one backend of the target.
#[cfg(target_arch = "wasm32")]
pub fn backend_declaring(capability: wcmp_wasm_core::Capability) -> wcmp_wasm_core_web::Web {
    let _ = capability;
    backend()
}

/// The name of the backend of this run.
#[cfg(target_arch = "wasm32")]
pub fn name() -> &'static str {
    "web"
}
