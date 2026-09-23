//! The configuration an [`Engine`] is built from.
//!
//! [`Engine`]: crate::Engine

use crate::internal::EngineConfigInternal;
use wasmtime_environ::wasmparser::WasmFeatures;

/// The configuration an [`Engine`] is built from: which Component
/// Model features the translator accepts.
///
/// The polyfill validates a component with the gates Wasmtime
/// validates with, so a binary Wasmtime rejects is rejected here
/// too. The features the compatibility outlook commits to are on:
/// the core Component Model, asynchronous function types, `map<K,
/// V>`, fixed-length `list<T, N>`, and 64-bit memories. Every gated
/// or in-development feature is off, and a host opts into one with
/// the setter named as in Wasmtime's `Config`. Nested namespaces and
/// projections in extern names have no setter: Wasmtime rejects them
/// too.
///
/// [`Engine`]: crate::Engine
#[derive(Clone, Debug)]
pub struct EngineConfig {
    features: WasmFeatures,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            features: WasmFeatures::default()
                | WasmFeatures::CM_MAP
                | WasmFeatures::CM_FIXED_LENGTH_LISTS
                | WasmFeatures::CM64,
        }
    }
}

impl EngineConfig {
    /// A configuration with the default feature set.
    pub fn new() -> Self {
        Self::default()
    }

    fn set(&mut self, feature: WasmFeatures, enable: bool) -> &mut Self {
        self.features.set(feature, enable);
        self
    }

    /// Accept the `implements` annotation on plain-named instance
    /// imports and exports. Off by default, as in Wasmtime.
    pub fn wasm_component_model_implements(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_IMPLEMENTS, enable)
    }

    /// Accept `map<K, V>` types. On by default.
    pub fn wasm_component_model_map(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_MAP, enable)
    }

    /// Accept fixed-length `list<T, N>` types. On by default.
    pub fn wasm_component_model_fixed_length_lists(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_FIXED_LENGTH_LISTS, enable)
    }

    /// Accept 64-bit memories in canonical options. On by default.
    pub fn wasm_component_model_memory64(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM64, enable)
    }

    /// Accept the `error-context` type and its built-ins. Off by
    /// default, as in Wasmtime.
    pub fn wasm_component_model_error_context(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_ERROR_CONTEXT, enable)
    }

    /// Accept the GC canonical ABI option. Off by default, as in
    /// Wasmtime.
    pub fn wasm_component_model_gc(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_GC, enable)
    }

    /// Accept stackful asynchronous lifts. Off by default, as in
    /// Wasmtime.
    pub fn wasm_component_model_async_stackful(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_ASYNC_STACKFUL, enable)
    }

    /// Accept the thread built-ins. Off by default, as in Wasmtime.
    pub fn wasm_component_model_threading(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_THREADING, enable)
    }

    /// Accept the additional canonical options on the asynchronous
    /// built-ins. Off by default, as in Wasmtime.
    pub fn wasm_component_model_more_async_builtins(&mut self, enable: bool) -> &mut Self {
        self.set(WasmFeatures::CM_MORE_ASYNC_BUILTINS, enable)
    }
}

impl EngineConfigInternal for EngineConfig {
    fn wasm_features(&self) -> WasmFeatures {
        self.features
    }
}
