//! Baseline tests for the suspend provider an engine selects.
//!
//! An engine selects the provider that fills its suspend capability
//! once, when it is constructed, and answers which one through
//! `Engine::suspend_provider`. The host's opt-out on `EngineConfig`
//! comes ahead of every probe. The switch probe passes on the native
//! engine on x86_64 Linux, where Wasmtime implements the
//! stack-switching proposal, so the engine answers the
//! stack-switching provider there. The browser's backend declares host
//! suspension in a browser that offers JavaScript Promise Integration,
//! which the flake's Chromium does, so the engine answers the
//! host-suspension provider in the web lane. An engine on any other native platform answers that it has no
//! provider.

#![cfg(test)]

use wcmp::{Engine, EngineConfig, SuspendProviderKind};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// What an engine that is allowed a provider answers on this target.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
const SELECTED: SuspendProviderKind = SuspendProviderKind::StackSwitching;

/// What an engine that is allowed a provider answers on this target.
#[cfg(target_arch = "wasm32")]
const SELECTED: SuspendProviderKind = SuspendProviderKind::HostSuspension;

/// What an engine that is allowed a provider answers on this target.
#[cfg(not(any(
    target_arch = "wasm32",
    all(target_arch = "x86_64", target_os = "linux")
)))]
const SELECTED: SuspendProviderKind = SuspendProviderKind::None;

#[wcmp_macros::test]
fn it_answers_no_provider_when_the_config_turns_the_provider_off() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
#[wcmp_macros::test]
fn it_answers_the_stack_switching_provider_on_the_native_engine() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::StackSwitching,
        "the native engine runs the switch probe's thread through one \
         suspension to its end"
    );

    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&EngineConfig::new()))
        .expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::StackSwitching
    );
}

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
fn it_answers_the_host_suspension_provider_in_the_browser() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension,
        "the flake's Chromium has JavaScript Promise Integration, so \
         the browser backend declares host suspension, and the switch \
         probe fails there"
    );

    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&EngineConfig::new()))
        .expect("engine");
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension
    );
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_arch = "x86_64", target_os = "linux")
)))]
#[wcmp_macros::test]
fn it_answers_no_provider_by_default_where_no_probe_passes() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);

    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&EngineConfig::new()))
        .expect("engine");
    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
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
    assert_eq!(rebuilt.suspend_provider(), SELECTED);
}

#[path = "support/backend.rs"]
mod test_backend;
