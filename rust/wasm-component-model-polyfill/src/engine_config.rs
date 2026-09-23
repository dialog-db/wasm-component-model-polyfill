//! The configuration an [`Engine`] is built from.
//!
//! [`Engine`]: crate::Engine

use crate::internal::EngineConfigInternal;
use wasmtime_environ::wasmparser::WasmFeatures;

/// The configuration an [`Engine`] is built from: which Component
/// Model features the translator accepts, and how many list elements
/// one crossing may lift out of a guest.
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
/// The element bound is described at
/// [`EngineConfig::max_list_elements`].
///
/// [`Engine`]: crate::Engine
#[derive(Clone, Debug)]
pub struct EngineConfig {
    features: WasmFeatures,
    max_list_elements: usize,
}

/// The default bound on the list elements one crossing may lift:
/// 4 194 304, which is `1 << 22`. See
/// [`EngineConfig::max_list_elements`].
pub const DEFAULT_MAX_LIST_ELEMENTS: usize = 1 << 22;

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            features: WasmFeatures::default()
                | WasmFeatures::CM_MAP
                | WasmFeatures::CM_FIXED_LENGTH_LISTS
                | WasmFeatures::CM64,
            max_list_elements: DEFAULT_MAX_LIST_ELEMENTS,
        }
    }
}

impl EngineConfig {
    /// A configuration with the default feature set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bound the list elements one crossing may lift out of a guest
    /// to `limit`. The default is 4 194 304 (`1 << 22`).
    ///
    /// A lifted list holds one [`Val`](crate::Val) per element, which
    /// is several times what the element occupies in guest memory —
    /// about forty bytes on a 64-bit host and about twenty in a
    /// browser, against one byte for a `u8` — so a guest that hands
    /// the host a list as long as its memory is wide would have the
    /// host reserve many times that memory. The bound is an element
    /// count because that count is what the host's allocation grows
    /// with, and it reads the same on both targets; it plays the part
    /// Wasmtime's per-call host fuel plays, whose default of 128 MiB
    /// charges one `Val` apiece. The default admits a little over
    /// that on a 64-bit host and a little under it in a browser.
    ///
    /// Every list one crossing lifts counts against the bound — the
    /// arguments of one call, or the result of one — nested lists and
    /// the entries of a `map` included, and the list that would pass
    /// it fails with [`AbiCause::ListElementLimit`] before anything
    /// is reserved for its elements or read out of it. A string is
    /// not a list and does not count: it crosses as one host string
    /// of the guest's bytes. The bound applies to lifting alone; a
    /// list the host lowers is one it already holds.
    ///
    /// [`AbiCause::ListElementLimit`]: crate::AbiCause::ListElementLimit
    pub fn max_list_elements(&mut self, limit: usize) -> &mut Self {
        self.max_list_elements = limit;
        self
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

    fn list_element_bound(&self) -> usize {
        self.max_list_elements
    }
}
