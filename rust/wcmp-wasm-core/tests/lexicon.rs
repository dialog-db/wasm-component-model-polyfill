//! The capability lexicon against `wasmparser`'s feature set.

use wasmparser::WasmFeatures;
use wcmp_wasm_core::{Capabilities, Capability};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Each capability that is a Wasm feature, beside the `wasmparser` feature
/// it stands for.
const FEATURES: [(Capability, WasmFeatures); 9] = [
    (Capability::MultiMemory, WasmFeatures::MULTI_MEMORY),
    (Capability::Memory64, WasmFeatures::MEMORY64),
    (Capability::TailCall, WasmFeatures::TAIL_CALL),
    (Capability::Exceptions, WasmFeatures::EXCEPTIONS),
    (
        Capability::FunctionReferences,
        WasmFeatures::FUNCTION_REFERENCES,
    ),
    (Capability::Gc, WasmFeatures::GC),
    (Capability::RelaxedSimd, WasmFeatures::RELAXED_SIMD),
    (Capability::Threads, WasmFeatures::THREADS),
    (Capability::StackSwitching, WasmFeatures::STACK_SWITCHING),
];

/// The `wasmparser` feature whose field is named `name`. Each flag of the
/// feature set is its field's name in upper case.
fn wasmparser_feature(name: &str) -> Option<WasmFeatures> {
    WasmFeatures::from_name(&name.to_ascii_uppercase())
}

#[wcmp_macros::test]
fn it_names_each_wasm_feature_as_wasmparser_does() {
    for (capability, feature) in FEATURES {
        assert_eq!(
            wasmparser_feature(capability.name()),
            Some(feature),
            "`{capability}` is not the name of its wasmparser feature",
        );
    }
}

#[wcmp_macros::test]
fn it_takes_every_wasm_feature_of_the_lexicon_into_account() {
    let wasm_features: Vec<_> = Capability::ALL
        .into_iter()
        .filter(|capability| capability.is_wasm_feature())
        .collect();
    let checked: Vec<_> = FEATURES.iter().map(|(capability, _)| *capability).collect();
    assert_eq!(wasm_features, checked);
}

#[wcmp_macros::test]
fn it_gives_no_wasm_feature_name_to_host_suspension_or_a_reserved_name() {
    for capability in Capability::ALL
        .into_iter()
        .filter(|capability| !capability.is_wasm_feature())
    {
        assert_eq!(
            wasmparser_feature(capability.name()),
            None,
            "`{capability}` is a wasmparser feature, and the lexicon says it is not",
        );
    }
}

#[wcmp_macros::test]
fn it_reserves_fuel_epoch_interruption_and_resource_limits() {
    let reserved: Vec<_> = Capability::ALL
        .into_iter()
        .filter(|capability| capability.is_reserved())
        .map(Capability::name)
        .collect();
    assert_eq!(reserved, ["fuel", "epoch_interruption", "resource_limits"]);

    let declared: Capabilities = Capability::ALL.into_iter().collect();
    assert!(declared.iter().all(|capability| !capability.is_reserved()));
}

#[wcmp_macros::test]
fn it_finds_each_capability_by_its_name() {
    let names: Vec<_> = Capability::ALL.into_iter().map(Capability::name).collect();
    assert_eq!(
        names,
        [
            "multi_memory",
            "memory64",
            "tail_call",
            "exceptions",
            "function_references",
            "gc",
            "relaxed_simd",
            "threads",
            "stack_switching",
            "host_suspension",
            "fuel",
            "epoch_interruption",
            "resource_limits",
        ]
    );
    for capability in Capability::ALL {
        assert_eq!(Capability::from_name(capability.name()), Some(capability));
        assert_eq!(capability.to_string(), capability.name());
    }
    assert_eq!(Capability::from_name("simd"), None);
}
