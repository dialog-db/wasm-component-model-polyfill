// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill's compilation context.
//!
//! `Engine` is a thin wrapper over the runtime-layer engine the host
//! built from the backend of its choice. It hides the backend from
//! the polyfill's public API and is the moral equivalent of
//! `wasmtime::component::Engine` — the type from which component
//! compilation hangs. The runtime-layer engine sees only core
//! WebAssembly; component-level work is layered on top of the
//! runtime layer's abstractions in later modules and is not
//! delegated to a backend's component runtime.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::concurrency::SwitchProbe;
use crate::engine_config::EngineConfig;
use crate::error::Result;
use crate::internal::{EngineConfigInternal, EngineInternal};
use crate::runtime_layer::{
    Backend, Capability, Engine as RuntimeEngine, Module as RuntimeModule, Shared,
};
use crate::suspend_provider_kind::SuspendProviderKind;

/// The polyfill's compilation context.
///
/// An `Engine` carries the backend the host chose, and the
/// configuration shared by every component the polyfill compiles and
/// instantiates. It is cheap to clone — internal state is shared —
/// and is made only by [`Engine::with_backend`]: the polyfill has no
/// backend of its own, on any target, so the host always names one.
///
/// The engine carries the [`EngineConfig`] every component it
/// translates is validated with, and the suspend provider it
/// selected when it was constructed.
#[derive(Clone)]
pub struct Engine {
    inner: Shared<RuntimeEngine>,
    config: EngineConfig,
    suspend_provider: SuspendProviderKind,
    /// The switch modules the stores of this engine instantiate,
    /// compiled once each, by their bytes.
    switch_modules: Shared<SwitchModules>,
}

/// The switch modules of one engine, compiled once each, by their bytes.
type SwitchModules = Arc<Mutex<HashMap<Vec<u8>, RuntimeModule>>>;

impl Engine {
    /// Construct an `Engine` over `backend`, with the default
    /// [`EngineConfig`].
    ///
    /// The backend is a crate of the runtime layer that the host adds
    /// beside the polyfill: Wasmtime natively, or the browser's
    /// engine in a page. The engine holds it behind dynamic dispatch,
    /// so no type of the polyfill names it. Two engines over two
    /// backends live side by side in one program.
    ///
    /// Construction selects the suspend provider, synchronously, and
    /// the engine keeps that answer for its life: see
    /// [`suspend_provider`](Self::suspend_provider).
    ///
    /// The return type is [`Result`] for forward compatibility: no
    /// backend refuses an engine today, but later work will accept
    /// configuration that can fail at construction time.
    #[allow(clippy::unnecessary_wraps)]
    pub fn with_backend(backend: impl Backend) -> Result<Self> {
        let inner = Shared::new(RuntimeEngine::with_backend(backend));
        let config = EngineConfig::default();
        let suspend_provider = select(&inner, &config);
        Ok(Self {
            inner,
            config,
            suspend_provider,
            switch_modules: Shared::new(Arc::default()),
        })
    }

    /// The engine over the same backend, with `config` in place of
    /// the configuration it was made with: the polyfill's analogue to
    /// building a Wasmtime engine from a `Config`.
    ///
    /// The suspend provider is selected again for `config`, and the
    /// answer holds for the life of the engine that comes back. The
    /// engine this is called on keeps its own configuration, and so
    /// does every clone of it.
    #[allow(clippy::unnecessary_wraps)]
    pub fn with_config(self, config: &EngineConfig) -> Result<Self> {
        let suspend_provider = select(&self.inner, config);
        Ok(Self {
            config: config.clone(),
            suspend_provider,
            ..self
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
    /// [`SuspendProviderKind::StackSwitching`] where the backend
    /// declares stack switching and runs the switch probe, a thread
    /// that suspends and resumes through the WebAssembly
    /// stack-switching instructions, which the Wasmtime backend does
    /// on x86_64 Linux. It answers
    /// [`SuspendProviderKind::HostSuspension`] where the backend
    /// declares host suspension, which the browser's backend does in
    /// every current browser. It answers [`SuspendProviderKind::None`]
    /// on a backend that declares neither.
    ///
    /// Each store of an engine that selected a provider instantiates it
    /// as it is constructed, and runs its guest threads through it:
    /// each thread entry starts on a stack of its own, and a blocking
    /// built-in suspends that stack until the scheduler resumes it. The
    /// host-suspension provider resumes a thread when the driver of the
    /// store awaits it, and the scheduler runs nothing else until the
    /// thread stops again.
    ///
    /// Wasmtime has no counterpart, because its fibers always exist.
    ///
    /// [`SchedulerCause::StackSwitchNeeded`]: crate::SchedulerCause::StackSwitchNeeded
    pub fn suspend_provider(&self) -> SuspendProviderKind {
        self.suspend_provider
    }
}

impl EngineInternal for Engine {
    fn inner(&self) -> &RuntimeEngine {
        &self.inner
    }

    fn switch_modules(&self) -> &SwitchModules {
        &self.switch_modules
    }
}

/// Select the provider that fills the suspend capability of an engine
/// over `inner` configured with `config`.
fn select(inner: &RuntimeEngine, config: &EngineConfig) -> SuspendProviderKind {
    select_suspend_provider(
        config.suspend_provider_enabled(),
        || SwitchProbe::new().passes(inner),
        || inner.capabilities().contains(Capability::HostSuspension),
    )
}

/// Select the provider that fills the suspend capability of an
/// engine, in the order the engine fixes: the host's opt-out,
/// `enabled`, then the switch probe, then whether the backend
/// declares host suspension, then no provider.
///
/// The host's opt-out comes ahead of every probe, and a probe the
/// order does not reach never runs. The stack-switching provider
/// comes before the host-suspension provider because it resumes a
/// thread synchronously, inside the call that asks for it, so its
/// scheduling order matches the native order with no wait between
/// two items. No backend declares both today, so that order decides
/// nothing yet. Each probe is small and synchronous, so construction
/// stays synchronous.
fn select_suspend_provider(
    enabled: bool,
    switch_probe: impl FnOnce() -> bool,
    host_suspension: impl FnOnce() -> bool,
) -> SuspendProviderKind {
    if !enabled {
        return SuspendProviderKind::None;
    }
    if switch_probe() {
        return SuspendProviderKind::StackSwitching;
    }
    if host_suspension() {
        return SuspendProviderKind::HostSuspension;
    }
    SuspendProviderKind::None
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
    fn it_selects_the_stack_switching_provider_before_the_host_suspension_provider() {
        assert_eq!(
            select_suspend_provider(true, || true, unreached),
            SuspendProviderKind::StackSwitching,
            "a passing switch probe settles the answer, so the host-suspension probe \
             never runs"
        );
        assert_eq!(
            select_suspend_provider(true, || false, || true),
            SuspendProviderKind::HostSuspension
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
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let probe = SwitchProbe::over(NEVER_FINISHES);

        assert!(
            !probe.passes(engine.inner()),
            "a thread that suspends where it should finish fails the probe"
        );
        assert_eq!(
            select_suspend_provider(true, || probe.passes(engine.inner()), || false),
            SuspendProviderKind::None,
            "a failed probe selects no provider"
        );
    }

    #[wcmp_macros::test]
    fn it_fails_the_probe_for_a_module_the_engine_rejects() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");

        assert!(!SwitchProbe::over(b"\0asm\x01\0\0\0\x0d").passes(engine.inner()));
    }
}
