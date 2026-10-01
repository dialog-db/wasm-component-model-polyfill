//! The suite a script belongs to: the floor, or the capabilities it needs.

use core::fmt;

use wasmparser::{Validator, WasmFeatures};
use wast::parser;
use wast::{QuoteWat, Wast, WastDirective, WastExecute, Wat};
use wcmp_wasm_core::{Capabilities, Capability};

use crate::text;

/// The part of the fidelity suite that one script belongs to.
///
/// The validator decides. The modules a script expects to be valid are the
/// ones its `module` and `module definition` directives define, and the
/// ones it expects to fail only at link or at instantiation. The script
/// needs the least set of features above Wasm 2.0 under which every such
/// module validates. The validator's features have the names of the
/// capability lexicon, so the set reads as capabilities, unless it holds a
/// feature the lexicon does not name.
///
/// A script also asserts that some modules are refused. Under the features
/// it needs, the validator must refuse each of them too. A script that
/// asserts the refusal of a module its own suite makes valid contradicts
/// its suite, as a proposal's copy of the test suite does where it predates
/// Wasm 2.0, and no backend runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Suite {
    /// The capabilities the script needs above the floor. An empty set is
    /// the floor, Wasm 2.0.
    Capabilities(Capabilities),
    /// No backend runs the script. The text says why: it needs a feature
    /// the lexicon does not name, it contradicts its own suite, or it holds
    /// a module the validator refuses under every feature.
    Outside(String),
}

impl Suite {
    /// The suite of the script whose source is `source`, or why the script
    /// does not parse.
    pub fn of(source: &str) -> Result<Suite, String> {
        let modules = Modules::of(source)?;
        let all = WasmFeatures::all();
        if !modules.valid.iter().all(|bytes| validates(all, bytes)) {
            return Ok(Suite::Outside(
                "holds a module that the validator refuses under every feature".to_string(),
            ));
        }

        let features = least(WasmFeatures::WASM2, all, |features| {
            modules.valid.iter().all(|bytes| validates(features, bytes))
        });
        let mut capabilities = Capabilities::empty();
        let mut unnamed = Vec::new();
        for (name, _) in features.difference(WasmFeatures::WASM2).iter_names() {
            let name = name.to_ascii_lowercase();
            match Capability::from_name(&name).filter(|capability| capability.is_wasm_feature()) {
                Some(capability) => capabilities = capabilities.with(capability),
                None => unnamed.push(name),
            }
        }
        if !unnamed.is_empty() {
            return Ok(Suite::Outside(format!(
                "needs {}, which the lexicon does not name",
                unnamed.join(", ")
            )));
        }

        if let Some(refusal) = modules
            .refusals
            .iter()
            .find(|refusal| validates(features, &refusal.bytes))
        {
            return Ok(Suite::Outside(format!(
                "asserts on line {} that a module is refused, which {} makes valid",
                refusal.line,
                Suite::Capabilities(capabilities).describe()
            )));
        }
        Ok(Suite::Capabilities(capabilities))
    }

    /// Whether a backend that declares `declared` runs the script: the
    /// script belongs to the floor or to capabilities the backend declares.
    pub fn runs_with(&self, declared: Capabilities) -> bool {
        match self {
            Suite::Capabilities(needed) => needed.iter().all(|needed| declared.contains(needed)),
            Suite::Outside(_) => false,
        }
    }

    /// The capabilities of `declared` that lift the refusal of the module
    /// `bytes`, which a script that needs `needed` asserts: the least of
    /// them that make the module valid, where the script's own suite does
    /// not.
    ///
    /// A script asserts the refusal of a module under the features the
    /// script needs. A capability the backend declares above those can make
    /// the module valid, as stack switching does a tag with results, and a
    /// faithful engine then accepts it. The refusal does not hold on that
    /// backend, and the runner skips it, naming these capabilities. A module
    /// that the script's own suite makes valid is no lifted refusal: the
    /// directive runs, and a faithful engine fails it.
    pub fn lifting(
        declared: Capabilities,
        needed: Capabilities,
        bytes: &[u8],
    ) -> Option<Capabilities> {
        let own = features(needed);
        let most = own.union(features(declared));
        if validates(own, bytes) || !validates(most, bytes) {
            return None;
        }
        let least = least(own, most, |features| validates(features, bytes));
        Some(
            declared
                .iter()
                .filter(|capability| capability.is_wasm_feature() && !needed.contains(*capability))
                .filter(|capability| least.contains(flag(*capability)))
                .collect(),
        )
    }

    /// The refusals of the script `source` that the capabilities of
    /// `declared` lift, each by its line and the capabilities that lift it.
    /// A script outside the suite lifts none.
    pub fn lifted_refusals(
        source: &str,
        declared: Capabilities,
    ) -> Result<Vec<(usize, Capabilities)>, String> {
        let Suite::Capabilities(needed) = Suite::of(source)? else {
            return Ok(Vec::new());
        };
        let modules = Modules::of(source)?;
        Ok(modules
            .refusals
            .iter()
            .filter_map(|refusal| {
                Suite::lifting(declared, needed, &refusal.bytes)
                    .map(|lifting| (refusal.line, lifting))
            })
            .collect())
    }

    /// The features of the suite in words, such as `Wasm 2.0 with gc`.
    fn describe(&self) -> String {
        match self {
            Suite::Capabilities(capabilities) if *capabilities == Capabilities::empty() => {
                "Wasm 2.0".to_string()
            }
            suite => format!("Wasm 2.0 with {suite}"),
        }
    }
}

impl fmt::Display for Suite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Suite::Capabilities(capabilities) if *capabilities == Capabilities::empty() => {
                f.write_str("floor")
            }
            Suite::Capabilities(capabilities) => {
                let names = capabilities
                    .iter()
                    .map(Capability::name)
                    .collect::<Vec<_>>();
                f.write_str(&names.join(", "))
            }
            Suite::Outside(why) => write!(f, "outside the suite: {why}"),
        }
    }
}

/// The features of the floor and of each capability of `capabilities`.
fn features(capabilities: Capabilities) -> WasmFeatures {
    capabilities
        .iter()
        .fold(WasmFeatures::WASM2, |features, capability| {
            features.union(flag(capability))
        })
}

/// The validator's feature of the same name as `capability`, or none for
/// a name that is not a Wasm feature.
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

/// A module a script asserts is refused, malformed or invalid.
struct Refusal {
    /// The line of the directive, counted from one.
    line: usize,
    bytes: Vec<u8>,
}

/// The modules of a script, as the validator reads them. A module whose
/// text does not encode is left out: a valid one fails its directive in the
/// run, whatever its suite, and a refused one is refused before an engine
/// sees it.
struct Modules {
    /// The binary of every module the script expects to be valid.
    valid: Vec<Vec<u8>>,
    /// Every module the script expects to be refused.
    refusals: Vec<Refusal>,
}

impl Modules {
    fn of(source: &str) -> Result<Self, String> {
        let buffer = text::buffer(source).map_err(|error| error.to_string())?;
        let mut wast = parser::parse::<Wast<'_>>(&buffer).map_err(|error| error.to_string())?;
        let mut modules = Modules {
            valid: Vec::new(),
            refusals: Vec::new(),
        };
        modules.collect(source, &mut wast.directives);
        Ok(modules)
    }

    fn collect(&mut self, source: &str, directives: &mut [WastDirective<'_>]) {
        for directive in directives {
            let line = text::line_of(source, directive.span());
            match directive {
                WastDirective::Module(module) | WastDirective::ModuleDefinition(module) => {
                    if let QuoteWat::Wat(Wat::Module(_)) | QuoteWat::QuoteModule(..) = module
                        && let Ok(bytes) = module.encode()
                    {
                        self.valid.push(bytes);
                    }
                }
                WastDirective::AssertTrap { exec, .. }
                | WastDirective::AssertReturn { exec, .. }
                | WastDirective::AssertException { exec, .. }
                | WastDirective::AssertSuspension { exec, .. } => {
                    if let WastExecute::Wat(wat @ Wat::Module(_)) = exec
                        && let Ok(bytes) = wat.encode()
                    {
                        self.valid.push(bytes);
                    }
                }
                WastDirective::AssertUnlinkable { module, .. } => {
                    if let Wat::Module(_) = module
                        && let Ok(bytes) = module.encode()
                    {
                        self.valid.push(bytes);
                    }
                }
                WastDirective::AssertMalformed { module, .. }
                | WastDirective::AssertInvalid { module, .. } => {
                    if let QuoteWat::Wat(Wat::Module(_)) | QuoteWat::QuoteModule(..) = module
                        && let Ok(bytes) = module.encode()
                    {
                        self.refusals.push(Refusal { line, bytes });
                    }
                }
                WastDirective::Thread(thread) => self.collect(source, &mut thread.directives),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_puts_a_script_of_wasm_2_0_on_the_floor() {
        let suite = Suite::of(
            r#"
            (module (func (export "f") (result v128) (v128.const i32x4 0 0 0 0)))
            (assert_return (invoke "f") (v128.const i32x4 0 0 0 0))
            (assert_invalid (module (func (result i32))) "type mismatch")
            "#,
        )
        .expect("the script parses");
        assert_eq!(suite, Suite::Capabilities(Capabilities::empty()));
        assert_eq!(suite.to_string(), "floor");
        assert!(suite.runs_with(Capabilities::empty()));
    }

    #[wcmp_macros::test]
    fn it_names_the_least_capabilities_the_modules_need() {
        let suite = Suite::of(
            r#"
            (module (memory 1) (memory 1))
            (module (type $t (struct (field i32)))
              (func (export "f") (result (ref null $t)) (ref.null $t)))
            (assert_unlinkable
              (module (import "m" "f" (func)) (func (return_call 0)))
              "unknown import")
            "#,
        )
        .expect("the script parses");
        let needed = [
            Capability::MultiMemory,
            Capability::TailCall,
            Capability::Gc,
        ]
        .into_iter()
        .collect::<Capabilities>();
        assert_eq!(suite, Suite::Capabilities(needed));
        assert_eq!(suite.to_string(), "multi_memory, tail_call, gc");
        assert!(suite.runs_with(needed.with(Capability::Threads)));
        assert!(!suite.runs_with(needed.without(Capability::Gc)));
    }

    #[wcmp_macros::test]
    fn it_leaves_a_script_that_needs_a_feature_outside_the_lexicon_to_no_backend() {
        let suite = Suite::of(r#"(module (global i32 (i32.add (i32.const 1) (i32.const 2))))"#)
            .expect("the script parses");
        assert_eq!(
            suite,
            Suite::Outside("needs extended_const, which the lexicon does not name".to_string())
        );
        assert!(!suite.runs_with(Capability::ALL.into_iter().collect()));
    }

    #[wcmp_macros::test]
    fn it_leaves_a_script_that_contradicts_its_own_suite_to_no_backend() {
        // Wasm 2.0 allows two tables, so a script written before it
        // contradicts the floor.
        let suite = Suite::of(
            r#"
            (module (memory 1 1 shared))
            (assert_invalid (module (table 0 funcref) (table 0 funcref)) "multiple tables")
            "#,
        )
        .expect("the script parses");
        assert_eq!(
            suite,
            Suite::Outside(
                "asserts on line 3 that a module is refused, which Wasm 2.0 with threads makes \
                 valid"
                    .to_string()
            )
        );
    }

    #[wcmp_macros::test]
    fn it_names_the_declared_capability_that_lifts_a_refusal() {
        let source = r#"
            (module (tag (param i32)))
            (assert_invalid (module (tag (result i32))) "non-empty tag result type")
            (assert_invalid (module (func (result i32))) "type mismatch")
            "#;
        assert_eq!(
            Suite::of(source),
            Ok(Suite::Capabilities(
                Capabilities::empty().with(Capability::Exceptions)
            ))
        );
        let declared = Capability::ALL.into_iter().collect::<Capabilities>();
        assert_eq!(
            Suite::lifted_refusals(source, declared),
            Ok(vec![(
                3,
                Capabilities::empty().with(Capability::StackSwitching)
            )])
        );
        assert_eq!(
            Suite::lifted_refusals(source, declared.without(Capability::StackSwitching)),
            Ok(Vec::new())
        );
    }
}
