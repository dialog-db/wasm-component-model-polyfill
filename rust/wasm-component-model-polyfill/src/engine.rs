//! The polyfill's compilation context.
//!
//! `Engine` is a thin wrapper over the runtime-layer engine selected
//! at compile time for the host platform. It hides the backend type
//! from the polyfill's public API and is the moral equivalent of
//! `wasmtime::component::Engine` — the type from which component
//! compilation hangs. The runtime-layer engine sees only core
//! WebAssembly; component-level work is layered on top of the
//! runtime-layer's generic abstractions in later modules and is not
//! delegated to a backend's component runtime.

use crate::backend::Backend;
use crate::engine_config::EngineConfig;
use crate::error::Result;
use crate::internal::{EngineConfigInternal, EngineInternal};
use crate::suspend_provider_kind::SuspendProviderKind;

/// The polyfill's compilation context.
///
/// An `Engine` carries the configuration shared by every component the
/// polyfill compiles and instantiates. It is cheap to clone — internal
/// state is shared — and is constructed without arguments via
/// [`Engine::new`].
///
/// The engine carries the [`EngineConfig`] every component it
/// translates is validated with, and the suspend provider it
/// selected when it was constructed.
#[derive(Clone)]
pub struct Engine {
    inner: wasm_runtime_layer::Engine<Backend>,
    config: EngineConfig,
    suspend_provider: SuspendProviderKind,
}

impl Engine {
    /// Construct an `Engine` over a default-configured backend, with
    /// the default [`EngineConfig`].
    ///
    /// The return type is [`Result`] for forward compatibility:
    /// today, both supported backends are infallibly default-
    /// constructible, but later work will accept configuration that
    /// can fail at construction time.
    pub fn new() -> Result<Self> {
        Self::with_config(&EngineConfig::default())
    }

    /// Construct an `Engine` from `config`, the polyfill's analogue
    /// to building a Wasmtime engine from a `Config`.
    ///
    /// Construction selects the suspend provider, synchronously, and
    /// the engine keeps that answer for its life: see
    /// [`suspend_provider`](Self::suspend_provider).
    #[allow(clippy::unnecessary_wraps)]
    pub fn with_config(config: &EngineConfig) -> Result<Self> {
        Ok(Self {
            inner: wasm_runtime_layer::Engine::new(Backend::default()),
            config: config.clone(),
            suspend_provider: select_suspend_provider(config),
        })
    }

    /// The configuration this engine was built from.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Which provider fills this engine's suspend capability.
    ///
    /// The engine selected it once, when it was constructed, and the
    /// answer holds for the engine's life and every clone of it.
    /// [`SuspendProviderKind::None`] means a blocking built-in runs
    /// the waiting work in a nested turn above the blocked call, so
    /// a block whose releasing work lies on a frame below it fails
    /// with [`SchedulerCause::StackSwitchNeeded`]. A host reads the
    /// answer to explain that failure. The engine answers
    /// [`SuspendProviderKind::None`] when
    /// [`EngineConfig::suspend_provider`] turned the provider off,
    /// and on both targets today, because no probe exists yet.
    ///
    /// Wasmtime has no counterpart, because its fibers always exist.
    ///
    /// [`SchedulerCause::StackSwitchNeeded`]: crate::SchedulerCause::StackSwitchNeeded
    pub fn suspend_provider(&self) -> SuspendProviderKind {
        self.suspend_provider
    }
}

impl EngineInternal for Engine {
    fn inner(&self) -> &wasm_runtime_layer::Engine<Backend> {
        &self.inner
    }
}

/// Select the provider that fills the suspend capability of an
/// engine built from `config`.
///
/// The host's opt-out comes ahead of every probe. The
/// stack-switching provider comes before the JSPI provider because
/// it resumes a thread synchronously, so its scheduling order
/// matches the native order with no microtask between two items. No
/// engine offers both today, so that order decides nothing yet. Each
/// probe is small and synchronous, so construction stays
/// synchronous.
fn select_suspend_provider(config: &EngineConfig) -> SuspendProviderKind {
    if !config.suspend_provider_enabled() {
        return SuspendProviderKind::None;
    }
    if switch_probe_passes() {
        return SuspendProviderKind::StackSwitching;
    }
    if jspi_probe_passes() {
        return SuspendProviderKind::Jspi;
    }
    SuspendProviderKind::None
}

/// Whether the engine runs a thread that suspends and resumes
/// through the WebAssembly stack-switching instructions. The
/// polyfill has no switch module to probe with yet, so the probe
/// never passes.
fn switch_probe_passes() -> bool {
    false
}

/// Whether the browser offers JavaScript Promise Integration. The
/// polyfill has no JSPI provider to select yet, so the probe never
/// passes, on the native target or in the browser.
fn jspi_probe_passes() -> bool {
    false
}
