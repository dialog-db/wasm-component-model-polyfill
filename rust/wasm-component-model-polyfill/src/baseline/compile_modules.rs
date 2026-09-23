//! Baseline tests for what a failed compile leaves on the engine.
//!
//! In the browser the core modules of a component are compiled with
//! `WebAssembly.compile`, and each finished compile waits on the
//! engine, keyed by its bytes, for the constructor that builds the
//! module from it. When one compile of a component fails, the modules
//! that did compile are never built, and what these tests are about
//! is that their entries do not stay on the engine: an engine that
//! took a failed component holds what it held before.
//!
//! The native backend compiles synchronously and keeps nothing
//! between a compile and a constructor, so the measurement is the
//! browser's alone.

#![cfg(all(test, target_arch = "wasm32"))]

use crate::internal::EngineInternal;
use crate::{Component, Engine, Error};

/// How many `i32.const 1; drop` pairs the failing module's function
/// runs before the instruction the browser refuses: about a megabyte
/// and a half of code, so the browser's validation of it reaches the
/// failure well after the small module before it has compiled.
const FILLER: usize = 500_000;

/// A component of two core modules. The first is small and valid.
/// The second is valid to the translator but not to the browser: it
/// ends in `i64.add128`, from the wide-arithmetic proposal, which the
/// translator's validator accepts and the browser does not implement.
fn fails_in_second_module() -> Vec<u8> {
    let filler = "i32.const 1 drop\n".repeat(FILLER);
    let wat = format!(
        r#"(component
          (core module $small
            (func (export "one") (result i32) i32.const 1))
          (core module $wide
            (func (export "wide") (param i64 i64 i64 i64) (result i64 i64)
              {filler}
              local.get 0
              local.get 1
              local.get 2
              local.get 3
              i64.add128))
          (core instance (instantiate $small))
          (core instance (instantiate $wide)))"#
    );
    let buffer = wast::parser::ParseBuffer::new(&wat).expect("lex the component");
    let mut wat = wast::parser::parse::<wast::Wat>(&buffer).expect("parse the component");
    wat.encode().expect("encode the component")
}

/// How many compiled modules wait on `engine`'s backend for a
/// constructor.
fn precompiled_count(engine: &Engine) -> usize {
    engine.inner().clone().into_backend().precompiled_count()
}

#[wcmp_macros::test]
async fn it_leaves_no_compiled_module_on_the_engine_when_a_compile_fails() {
    let engine = Engine::new().expect("engine");
    let bytes = fails_in_second_module();

    let error = match Component::new(&engine, &bytes).await {
        Ok(_) => panic!("a core module the browser refuses fails the component"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::Instantiation(_)),
        "the browser's refusal of the second module is what failed, got {error:?}"
    );
    assert_eq!(
        precompiled_count(&engine),
        0,
        "the failed component left nothing on the engine, and failed with {error}"
    );
}
