// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The capability a module that Wasmi refuses needs.

use wasmparser::{Validator, WasmFeatures};
use wcmp_wasm_core::{Capabilities, Capability};

/// The capability that the module `bytes` needs and `declared` lacks, or
/// `None` where the module needs none: it is invalid under every feature,
/// or it validates without any capability the backend lacks.
///
/// The module needs the least set of features between the floor, with the
/// capabilities of `declared`, and every feature, under which it validates.
/// The validator's features have the names of the capability lexicon. Of
/// the capabilities in that set, the one named is the last in the lexicon,
/// which is the proposal that builds on the others: GC before the typed
/// function references it builds on, for example.
pub fn missing_capability(declared: Capabilities, bytes: &[u8]) -> Option<Capability> {
    let most = WasmFeatures::all();
    if !validates(most, bytes) {
        return None;
    }
    let floor = declared
        .iter()
        .fold(WasmFeatures::WASM2, |features, capability| {
            features.union(flag(capability))
        });
    let needed = least(floor, most, |features| validates(features, bytes));
    Capability::ALL
        .into_iter()
        .rev()
        .filter(|capability| capability.is_wasm_feature() && !declared.contains(*capability))
        .find(|capability| needed.contains(flag(*capability)))
}

/// The validator's feature of the same name as `capability`, or none for a
/// name that is not a Wasm feature.
fn flag(capability: Capability) -> WasmFeatures {
    WasmFeatures::from_name(&capability.name().to_ascii_uppercase())
        .unwrap_or_else(WasmFeatures::empty)
}

/// The least features between `floor` and `most` under which `holds`
/// holds, where it holds under `most`.
///
/// Each feature above the floor that can go goes. Later proposals build on
/// earlier ones, so the features go in the reverse of the validator's
/// order: garbage collection before the typed function references it
/// builds on, for example.
fn least(
    floor: WasmFeatures,
    most: WasmFeatures,
    holds: impl Fn(WasmFeatures) -> bool,
) -> WasmFeatures {
    let mut candidates = most
        .iter_names()
        .filter(|(_, flag)| !floor.contains(*flag))
        .collect::<Vec<_>>();
    candidates.reverse();
    let mut features = most;
    for (_, flag) in candidates {
        let fewer = features.difference(flag).union(floor);
        if holds(fewer) {
            features = fewer;
        }
    }
    features
}

/// Whether the module `bytes` validates under `features`.
fn validates(features: WasmFeatures, bytes: &[u8]) -> bool {
    Validator::new_with_features(features)
        .validate_all(bytes)
        .is_ok()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use wcmp_macros::wasm;

    use super::*;

    /// What the Wasmi backend declares.
    fn declared() -> Capabilities {
        [
            Capability::MultiMemory,
            Capability::Memory64,
            Capability::TailCall,
            Capability::RelaxedSimd,
        ]
        .into_iter()
        .collect()
    }

    #[wcmp_macros::test]
    fn it_names_the_capability_a_module_needs_above_what_is_declared() {
        let cases: [(&[u8], Capability); 4] = [
            (
                wasm!(r#"(module (type (struct (field i32))))"#),
                Capability::Gc,
            ),
            (
                wasm!(r#"(module (tag (param i32)))"#),
                Capability::Exceptions,
            ),
            (
                wasm!(r#"(module (type $f (func)) (func (param (ref $f))))"#),
                Capability::FunctionReferences,
            ),
            (
                wasm!(r#"(module (memory 1 1 shared))"#),
                Capability::Threads,
            ),
        ];
        for (bytes, capability) in cases {
            assert_eq!(missing_capability(declared(), bytes), Some(capability));
        }
    }

    #[wcmp_macros::test]
    fn it_names_the_proposal_that_builds_on_the_others() {
        let bytes = wasm!(
            r#"
            (module
              (type $box (struct (field i32)))
              (tag (param (ref null $box))))
            "#
        );
        assert_eq!(missing_capability(declared(), bytes), Some(Capability::Gc));
    }

    #[wcmp_macros::test]
    fn it_names_nothing_for_a_module_no_capability_makes_valid() {
        let invalid = wasm!(r#"(module (func (result i32)))"#);
        assert_eq!(missing_capability(declared(), invalid), None);
        let declared_only = wasm!(r#"(module (memory 1) (memory 1))"#);
        assert_eq!(missing_capability(declared(), declared_only), None);
    }
}
