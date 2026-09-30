//! Baseline tests for the suspend provider an engine selects.
//!
//! An engine selects the provider that fills its suspend capability
//! once, when it is constructed, and answers which one through
//! `Engine::suspend_provider`. The host's opt-out on `EngineConfig`
//! comes ahead of every probe. The switch probe passes on an engine over
//! the Wasmtime backend on x86_64 Linux, where Wasmtime implements the
//! stack-switching proposal, so the engine answers the stack-switching
//! provider there. The browser's backend declares host suspension in a
//! browser that offers JavaScript Promise Integration, which the
//! flake's Chromium does, so the engine answers the host-suspension
//! provider in the web lane. The Wasmi backend declares host suspension
//! on every platform, and the switch probe fails on it, so an engine over
//! it answers the host-suspension provider in the Wasmi lane. An engine
//! over Wasmtime on any other native platform answers that it has no
//! provider.

#![cfg(test)]

use wcmp::{Engine, EngineConfig, SuspendProviderKind};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// What an engine that is allowed a provider answers on this target,
/// over the backend of the run.
fn selected() -> SuspendProviderKind {
    if cfg!(target_arch = "wasm32") || crate::test_backend::name() == "wasmi" {
        SuspendProviderKind::HostSuspension
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        SuspendProviderKind::StackSwitching
    } else {
        SuspendProviderKind::None
    }
}

#[wcmp_macros::test]
fn it_answers_no_provider_when_the_config_turns_the_provider_off() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
}

#[wcmp_macros::test]
fn it_answers_the_provider_its_target_and_backend_offer() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        selected(),
        "an engine over `{}` selects what the backend and the target offer",
        crate::test_backend::name()
    );

    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&EngineConfig::new()))
        .expect("engine");
    assert_eq!(engine.suspend_provider(), selected());
}

#[wcmp_macros::test]
fn it_keeps_the_answer_it_selected_at_construction() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");

    // Turning the provider back on in the host's configuration
    // reaches neither the engine already built from it nor a clone
    // of that engine: the selection ran once, in construction.
    config.suspend_provider(true);
    let clone = engine.clone();

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
    assert_eq!(clone.suspend_provider(), SuspendProviderKind::None);

    // And an engine built from the configuration now selects what
    // this target offers.
    let rebuilt = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    assert_eq!(rebuilt.suspend_provider(), selected());
}

#[path = "support/backend.rs"]
mod test_backend;
