//! The Wasm features the translator validates a component with.
//!
//! The runtime layer's floor is Wasm 2.0, and every Wasm feature above
//! it is a capability a backend declares. The translator validates a
//! component with the floor and with the capabilities the backend
//! declares, so it accepts only core code the backend can compile. The
//! Component Model features come from the host's [`EngineConfig`]; the
//! lexicon names no Component Model feature.
//!
//! The features decide what the translator emits as well as what it
//! accepts. Wasmtime's fused adapter compiler wraps the body of each
//! adapter in an exception barrier, a `try_table` whose `catch_all`
//! traps, only where the features include exception handling. Over a
//! backend without `exceptions` the adapters carry no barrier, and the
//! backend can compile them.
//!
//! Multi-memory is the one exception to the rule. The translator
//! validates each adapter module it generates with the component's
//! features, and it panics on an adapter that fails: an adapter that
//! copies between the memories of two components uses two memories.
//! The translator therefore always runs with multi-memory on, and
//! [`require_single_memories`] refuses a module with two memories
//! before any module compiles, where the backend lacks
//! `multi_memory`.
//!
//! When the translator refuses a component that it would accept with
//! the capabilities the backend lacks, [`missing_capability`] names
//! one of them, so `Component::new` fails with `Unsupported` and that
//! name rather than with a validation error.
//!
//! [`EngineConfig`]: crate::EngineConfig

use wasmtime_environ::wasmparser::{Validator, WasmFeatures};

use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::runtime_layer::{Capabilities, Capability};

/// The flag of `capability` in `wasmparser`'s feature set, or `None`
/// for a name of the lexicon that is not a Wasm feature. Each name of
/// the lexicon is the name `wasmparser` gives the same feature.
fn feature_of(capability: Capability) -> Option<WasmFeatures> {
    Some(match capability {
        Capability::MultiMemory => WasmFeatures::MULTI_MEMORY,
        Capability::Memory64 => WasmFeatures::MEMORY64,
        Capability::TailCall => WasmFeatures::TAIL_CALL,
        Capability::Exceptions => WasmFeatures::EXCEPTIONS,
        Capability::FunctionReferences => WasmFeatures::FUNCTION_REFERENCES,
        Capability::Gc => WasmFeatures::GC,
        Capability::RelaxedSimd => WasmFeatures::RELAXED_SIMD,
        Capability::Threads => WasmFeatures::THREADS,
        Capability::StackSwitching => WasmFeatures::STACK_SWITCHING,
        _ => return None,
    })
}

/// The features the translator validates with over a backend that
/// declares `capabilities`, where the host's configuration selects
/// `configured`.
///
/// Each Wasm feature of the lexicon is on exactly where the backend
/// declares it, whatever `configured` says of it, and the floor is
/// always on. Multi-memory is on too, for the adapters the translator
/// generates: see [`require_single_memories`]. Every other feature,
/// the Component Model features among them, keeps the value
/// `configured` gives it.
pub fn translator_features(configured: WasmFeatures, capabilities: Capabilities) -> WasmFeatures {
    let mut features = configured | WasmFeatures::WASM2;
    for capability in Capability::ALL {
        if let Some(feature) = feature_of(capability) {
            features.set(feature, capabilities.contains(capability));
        }
    }
    features | WasmFeatures::MULTI_MEMORY
}

/// The capability the backend lacks that the component in `bytes`
/// needs, where the translator refused it with `features`. `None`
/// where the component is refused for another reason: it validates
/// with `features`, or it fails to validate even with every
/// capability the backend lacks.
///
/// Where the component needs more than one missing capability, the
/// search names the last of them in the order of the lexicon, so a
/// capability that builds on an earlier one is the one named: a
/// module of a GC language needs `function_references` as well as
/// `gc`, and fails with `gc`. This runs only on a refusal, and each
/// step validates the whole component once more.
pub fn missing_capability(
    bytes: &[u8],
    features: WasmFeatures,
    capabilities: Capabilities,
) -> Option<Capability> {
    if validates(bytes, features) {
        return None;
    }
    let missing: Vec<(Capability, WasmFeatures)> = Capability::ALL
        .into_iter()
        .filter(|capability| !capabilities.contains(*capability))
        .filter_map(|capability| feature_of(capability).map(|feature| (capability, feature)))
        .filter(|(_, feature)| !features.contains(*feature))
        .collect();
    let every = missing
        .iter()
        .fold(features, |features, (_, feature)| features | *feature);
    if !validates(bytes, every) {
        return None;
    }
    missing
        .iter()
        .rev()
        .find(|(_, feature)| !validates(bytes, every - *feature))
        .or(missing.last())
        .map(|(capability, _)| *capability)
}

/// Whether the component in `bytes` validates with `features`.
fn validates(bytes: &[u8], features: WasmFeatures) -> bool {
    Validator::new_with_features(features)
        .validate_all(bytes)
        .is_ok()
}

/// Refuse the modules of a component, over a backend that declares
/// `capabilities`, where one of them has more than one memory and the
/// backend lacks `multi_memory`. `memories` gives the number of
/// memories of each module, imported ones included: the modules the
/// component carries, and the adapters the translator generated.
///
/// The translator always validates with multi-memory on, so this is
/// where a module with two memories meets a backend without
/// `multi_memory`. It runs before any module compiles.
pub fn require_single_memories(
    capabilities: Capabilities,
    mut memories: impl Iterator<Item = usize>,
) -> Result<()> {
    if !capabilities.contains(Capability::MultiMemory) && memories.any(|count| count > 1) {
        return Err(Error::unsupported(Capability::MultiMemory.name()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use wasmtime_environ::component::{ComponentTypesBuilder, Translator};
    use wasmtime_environ::wasmparser::{Operator, Parser, Payload};
    use wasmtime_environ::{ScopeVec, Tunables};
    use wcmp_macros::component;

    use super::*;
    use crate::engine_config::EngineConfig;
    use crate::internal::EngineConfigInternal;

    /// The capabilities of a backend like Wasmi: no exception
    /// handling, no GC, and no typed function references.
    fn without_exceptions() -> Capabilities {
        [
            Capability::MultiMemory,
            Capability::Memory64,
            Capability::TailCall,
            Capability::RelaxedSimd,
            Capability::HostSuspension,
        ]
        .into_iter()
        .collect()
    }

    /// Every Wasm feature of the lexicon.
    fn every_feature() -> Capabilities {
        Capability::ALL
            .into_iter()
            .filter(|capability| feature_of(*capability).is_some())
            .collect()
    }

    /// Two inner components, where `$B` calls the function `$A`
    /// exports. The call passes through a fused adapter, which moves
    /// only numbers and so touches no memory.
    const COMPOSITION: &[u8] = component!(
        r#"
        (component
          (component $A
            (core module $m
              (func (export "double") (param i32) (result i32)
                local.get 0 i32.const 2 i32.mul))
            (core instance $i (instantiate $m))
            (func (export "double") (param "x" u32) (result u32)
              (canon lift (core func $i "double"))))
          (component $B
            (import "double" (func $double (param "x" u32) (result u32)))
            (core func $core-double (canon lower (func $double)))
            (core module $m
              (import "" "double" (func $double (param i32) (result i32)))
              (func (export "run") (param i32) (result i32)
                local.get 0 call $double))
            (core instance $i (instantiate $m
              (with "" (instance (export "double" (func $core-double))))))
            (func (export "run") (param "x" u32) (result u32)
              (canon lift (core func $i "run"))))
          (instance $a (instantiate $A))
          (instance $b (instantiate $B (with "double" (func $a "double"))))
          (export "run" (func $b "run")))
        "#
    );

    /// Two inner components, where `$B` passes a string to the
    /// function `$A` exports. The fused adapter copies the string from
    /// the memory of `$B` into the memory of `$A`, so it imports both.
    const TWO_MEMORY_COMPOSITION: &[u8] = component!(
        r#"
        (component
          (component $A
            (core module $m
              (memory (export "memory") 1)
              (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
                i32.const 1024)
              (func (export "take") (param i32 i32)))
            (core instance $i (instantiate $m))
            (func (export "take") (param "s" string)
              (canon lift (core func $i "take")
                (memory (core memory $i "memory"))
                (realloc (core func $i "cabi_realloc")))))
          (component $B
            (import "take" (func $take (param "s" string)))
            (core module $memory
              (memory (export "memory") 1))
            (core instance $mem (instantiate $memory))
            (core func $core-take
              (canon lower (func $take) (memory (core memory $mem "memory"))))
            (core module $m
              (import "" "take" (func $take (param i32 i32)))
              (func (export "run")
                i32.const 0 i32.const 0 call $take))
            (core instance $i (instantiate $m
              (with "" (instance (export "take" (func $core-take))))))
            (func (export "run")
              (canon lift (core func $i "run"))))
          (instance $a (instantiate $A))
          (instance $b (instantiate $B (with "take" (func $a "take"))))
          (export "run" (func $b "run")))
        "#
    );

    /// A component whose one core module allocates a GC struct.
    const GC_COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (type $cell (struct (field i32)))
            (func (export "run") (result i32)
              i32.const 7
              struct.new $cell
              struct.get $cell 0))
          (core instance $i (instantiate $m))
          (func (export "run") (result u32)
            (canon lift (core func $i "run"))))
        "#
    );

    /// The features the translator validates with over a backend that
    /// declares `capabilities`, with the default configuration.
    fn features(capabilities: Capabilities) -> WasmFeatures {
        translator_features(EngineConfig::default().wasm_features(), capabilities)
    }

    /// The core modules one translation of `bytes` with `features`
    /// produces, the adapters among them: the bytes and the number of
    /// memories of each.
    fn translated_modules(bytes: &[u8], features: WasmFeatures) -> Vec<(Vec<u8>, usize)> {
        let scope = ScopeVec::new();
        let tunables = Tunables::default_u32();
        let mut validator = Validator::new_with_features(features);
        let mut types = ComponentTypesBuilder::new(&validator);
        let (_, modules) = Translator::new(&tunables, &mut validator, &mut types, &scope)
            .translate(bytes)
            .expect("the component translates");
        modules
            .values()
            .map(|module| (module.wasm.to_vec(), module.module.memories.len()))
            .collect()
    }

    /// Whether any function of the core module in `bytes` has a
    /// `try_table`, the instruction an exception barrier opens with.
    fn has_try_table(bytes: &[u8]) -> bool {
        Parser::new(0).parse_all(bytes).any(|payload| {
            let Ok(Payload::CodeSectionEntry(body)) = payload else {
                return false;
            };
            body.get_operators_reader()
                .expect("the body has operators")
                .into_iter()
                .any(|operator| matches!(operator, Ok(Operator::TryTable { .. })))
        })
    }

    #[wcmp_macros::test]
    fn it_enables_each_capability_the_backend_declares_and_no_other() {
        let features = features(without_exceptions());

        for capability in [
            Capability::MultiMemory,
            Capability::Memory64,
            Capability::TailCall,
            Capability::RelaxedSimd,
        ] {
            let feature = feature_of(capability).expect("a Wasm feature");
            assert!(features.contains(feature), "{capability} is declared");
        }
        for capability in [
            Capability::Exceptions,
            Capability::FunctionReferences,
            Capability::Gc,
            Capability::Threads,
            Capability::StackSwitching,
        ] {
            let feature = feature_of(capability).expect("a Wasm feature");
            assert!(!features.contains(feature), "{capability} is not declared");
        }
        assert!(features.contains(WasmFeatures::WASM2), "the floor is on");
    }

    #[wcmp_macros::test]
    fn it_follows_the_backend_and_not_the_configuration_for_a_capability() {
        let configured = EngineConfig::default().wasm_features();
        assert!(
            !configured.contains(WasmFeatures::STACK_SWITCHING),
            "the configuration leaves stack switching off"
        );
        assert!(configured.contains(WasmFeatures::GC), "and GC on");

        let features = translator_features(configured, every_feature());
        assert!(features.contains(WasmFeatures::STACK_SWITCHING));

        let features = translator_features(configured, Capabilities::empty());
        assert!(!features.contains(WasmFeatures::GC));
    }

    #[wcmp_macros::test]
    fn it_keeps_multi_memory_on_for_the_adapters_the_translator_generates() {
        let features = features(Capabilities::empty());

        assert!(features.contains(WasmFeatures::MULTI_MEMORY));
    }

    #[wcmp_macros::test]
    fn it_keeps_the_component_model_features_of_the_configuration() {
        let mut config = EngineConfig::default();
        config.wasm_component_model_map(false);
        config.wasm_component_model_error_context(true);

        let features = translator_features(config.wasm_features(), every_feature());

        assert!(!features.contains(WasmFeatures::CM_MAP));
        assert!(features.contains(WasmFeatures::CM_ERROR_CONTEXT));
        assert!(features.contains(WasmFeatures::COMPONENT_MODEL));
    }

    #[wcmp_macros::test]
    fn it_translates_a_composition_without_the_exception_barrier_where_exceptions_are_absent() {
        let modules = translated_modules(COMPOSITION, features(without_exceptions()));

        assert!(
            modules.iter().all(|(bytes, _)| !has_try_table(bytes)),
            "no module, the adapter included, opens a barrier"
        );
    }

    #[wcmp_macros::test]
    fn it_translates_a_composition_with_the_exception_barrier_where_exceptions_are_declared() {
        let modules = translated_modules(
            COMPOSITION,
            features(without_exceptions().with(Capability::Exceptions)),
        );

        assert!(
            modules.iter().any(|(bytes, _)| has_try_table(bytes)),
            "the adapter opens a barrier"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_two_memory_adapter_where_the_backend_lacks_multi_memory() {
        let capabilities = every_feature().without(Capability::MultiMemory);
        let modules = translated_modules(TWO_MEMORY_COMPOSITION, features(capabilities));
        assert!(
            modules.iter().any(|(_, memories)| *memories == 2),
            "the adapter imports the memories of both components"
        );

        let error = require_single_memories(capabilities, modules.iter().map(|(_, count)| *count))
            .expect_err("the backend cannot compile the adapter");

        assert!(
            matches!(&error, Error::Unsupported { feature } if feature == "multi_memory"),
            "{error:?}"
        );
    }

    #[wcmp_macros::test]
    fn it_accepts_a_two_memory_adapter_where_the_backend_declares_multi_memory() {
        let modules = translated_modules(TWO_MEMORY_COMPOSITION, features(every_feature()));

        require_single_memories(every_feature(), modules.iter().map(|(_, count)| *count))
            .expect("the backend compiles the adapter");
    }

    #[wcmp_macros::test]
    fn it_names_the_capability_a_refused_component_needs() {
        let capabilities = without_exceptions();

        assert_eq!(
            missing_capability(GC_COMPONENT, features(capabilities), capabilities),
            Some(Capability::Gc)
        );
    }

    #[wcmp_macros::test]
    fn it_names_no_capability_for_a_component_that_validates() {
        let capabilities = without_exceptions();

        assert_eq!(
            missing_capability(COMPOSITION, features(capabilities), capabilities),
            None
        );
    }

    #[wcmp_macros::test]
    fn it_names_no_capability_for_a_component_that_no_capability_repairs() {
        let capabilities = without_exceptions();

        assert_eq!(
            missing_capability(
                b"\0asm\x0d\0\x01\0\xff",
                features(capabilities),
                capabilities
            ),
            None
        );
    }
}
