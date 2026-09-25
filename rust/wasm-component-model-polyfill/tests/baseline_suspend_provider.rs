//! Baseline tests for the suspend provider an engine selects.
//!
//! An engine selects the provider that fills its suspend capability
//! once, when it is constructed, and answers which one through
//! `Engine::suspend_provider`. The host's opt-out on `EngineConfig`
//! comes ahead of every probe. No probe exists yet, so an engine
//! answers that it has no provider on both targets, with the
//! provider allowed or not.

#![cfg(test)]

use wasm_component_model_polyfill::{Engine, EngineConfig, SuspendProviderKind};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
fn it_answers_no_provider_when_the_config_turns_the_provider_off() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_config(&config).expect("engine");

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
}

#[wcmp_macros::test]
fn it_answers_no_provider_by_default_while_no_probe_exists() {
    let engine = Engine::new().expect("engine");
    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);

    let engine = Engine::with_config(&EngineConfig::new()).expect("engine");
    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
}

#[wcmp_macros::test]
fn it_keeps_the_answer_it_selected_at_construction() {
    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_config(&config).expect("engine");

    // Turning the provider back on in the host's configuration
    // reaches neither the engine already built from it nor a clone
    // of that engine: the selection ran once, in construction.
    config.suspend_provider(true);
    let clone = engine.clone();

    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);
    assert_eq!(clone.suspend_provider(), SuspendProviderKind::None);
}
