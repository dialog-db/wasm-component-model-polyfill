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
use crate::concurrency::SwitchProbe;
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
        let inner = wasm_runtime_layer::Engine::new(Backend::default());
        let suspend_provider = select_suspend_provider(
            config.suspend_provider_enabled(),
            || SwitchProbe::new().passes(&inner),
            jspi_probe_passes,
        );
        Ok(Self {
            inner,
            config: config.clone(),
            suspend_provider,
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
    /// [`EngineConfig::suspend_provider`] turned the provider off.
    /// Otherwise it answers
    /// [`SuspendProviderKind::StackSwitching`] where the engine runs
    /// the switch probe, a thread that suspends and resumes through
    /// the WebAssembly stack-switching instructions, which the native
    /// engine does on x86_64 Linux. It answers
    /// [`SuspendProviderKind::None`] on every other target today,
    /// because the JSPI probe does not exist yet.
    ///
    /// The scheduler does not run guest threads through the selected
    /// provider yet: today a blocking built-in runs the waiting work in
    /// a nested turn whatever the answer is.
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
/// engine, in the order the engine fixes: the host's opt-out,
/// `enabled`, then the switch probe, then the JSPI probe, then no
/// provider.
///
/// The host's opt-out comes ahead of every probe, and a probe the
/// order does not reach never runs. The stack-switching provider
/// comes before the JSPI provider because it resumes a thread
/// synchronously, so its scheduling order matches the native order
/// with no microtask between two items. No engine offers both today,
/// so that order decides nothing yet. Each probe is small and
/// synchronous, so construction stays synchronous.
fn select_suspend_provider(
    enabled: bool,
    switch_probe: impl FnOnce() -> bool,
    jspi_probe: impl FnOnce() -> bool,
) -> SuspendProviderKind {
    if !enabled {
        return SuspendProviderKind::None;
    }
    if switch_probe() {
        return SuspendProviderKind::StackSwitching;
    }
    if jspi_probe() {
        return SuspendProviderKind::Jspi;
    }
    SuspendProviderKind::None
}

/// Whether the browser offers JavaScript Promise Integration. The
/// polyfill has no JSPI provider to select yet, so the probe never
/// passes, on the native target or in the browser.
fn jspi_probe_passes() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::concurrency::SwitchProbe;

    /// A probe the selection must not reach.
    fn unreached() -> bool {
        panic!("the selection ran a probe after its answer was settled")
    }

    #[wcmp_macros::test]
    fn it_selects_no_provider_ahead_of_every_probe_when_the_host_opts_out() {
        assert_eq!(
            select_suspend_provider(false, unreached, unreached),
            SuspendProviderKind::None
        );
    }

    #[wcmp_macros::test]
    fn it_selects_the_stack_switching_provider_before_the_jspi_provider() {
        assert_eq!(
            select_suspend_provider(true, || true, unreached),
            SuspendProviderKind::StackSwitching,
            "a passing switch probe settles the answer, so the JSPI probe \
             never runs"
        );
        assert_eq!(
            select_suspend_provider(true, || false, || true),
            SuspendProviderKind::Jspi
        );
        assert_eq!(
            select_suspend_provider(true, || false, || false),
            SuspendProviderKind::None
        );
    }

    /// The probe module with a thread that suspends a second time
    /// where it should return, so `run` answers that the thread
    /// suspended again rather than finished.
    const NEVER_FINISHES: &[u8] = wcmp_macros::wasm!(
        r#"
        (module
          (type (func))
          (type (cont 0))
          (tag (type 0))
          (func (type 0)
            suspend 0
            suspend 0)
          (elem declare func 0)
          (func (export "run") (result i32)
            (local (ref null 1))
            block (result (ref 1))
              ref.func 0
              cont.new 1
              resume 1 (on 0 0)
              i32.const 0
              return
            end
            local.set 0
            block (result (ref 1))
              local.get 0
              resume 1 (on 0 0)
              i32.const 1
              return
            end
            drop
            i32.const 2))
        "#
    );

    #[wcmp_macros::test]
    fn it_selects_no_provider_when_the_probe_thread_does_not_finish() {
        let engine = Engine::new().expect("engine");
        let probe = SwitchProbe::over(NEVER_FINISHES);

        assert!(
            !probe.passes(engine.inner()),
            "a thread that suspends where it should finish fails the probe"
        );
        assert_eq!(
            select_suspend_provider(true, || probe.passes(engine.inner()), jspi_probe_passes),
            SuspendProviderKind::None,
            "a failed probe selects no provider"
        );
    }

    #[wcmp_macros::test]
    fn it_fails_the_probe_for_a_module_the_engine_rejects() {
        let engine = Engine::new().expect("engine");

        assert!(!SwitchProbe::over(b"\0asm\x01\0\0\0\x0d").passes(engine.inner()));
    }
}
