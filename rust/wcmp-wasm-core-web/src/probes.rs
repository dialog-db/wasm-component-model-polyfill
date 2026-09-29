//! The probes the backend runs for each capability when it is made.

use js_sys::{Uint8Array, WebAssembly};
use wcmp_macros::wasm;
use wcmp_wasm_core::{Capabilities, Capability};

/// One small module for each Wasm feature of the lexicon that the browser
/// can implement. Each module uses the feature, and nothing else above the
/// floor.
const PROBES: [(Capability, &[u8]); 9] = [
    (
        Capability::MultiMemory,
        wasm!(r#"(module (memory 1) (memory 1))"#),
    ),
    (Capability::Memory64, wasm!(r#"(module (memory i64 1))"#)),
    (
        Capability::TailCall,
        wasm!(
            r#"
            (module
              (func $callee)
              (func return_call $callee))
            "#
        ),
    ),
    (
        Capability::Exceptions,
        wasm!(
            r#"
            (module
              (tag $oops)
              (func (result exnref)
                block $caught (result exnref)
                  try_table (catch_all_ref $caught)
                    throw $oops
                  end
                  unreachable
                end)
              (func (param exnref)
                local.get 0
                throw_ref))
            "#
        ),
    ),
    (
        Capability::FunctionReferences,
        wasm!(
            r#"
            (module
              (type $unary (func (param i32) (result i32)))
              (func (param i32 (ref $unary)) (result i32)
                local.get 0
                local.get 1
                call_ref $unary))
            "#
        ),
    ),
    (
        Capability::Gc,
        wasm!(
            r#"
            (module
              (type $cell (struct (field (mut i32))))
              (func (result i32)
                i32.const 7
                struct.new $cell
                struct.get $cell 0
                ref.i31
                i31.get_s))
            "#
        ),
    ),
    (
        Capability::RelaxedSimd,
        wasm!(
            r#"
            (module
              (func (param v128 v128 v128) (result v128)
                local.get 0
                local.get 1
                local.get 2
                f32x4.relaxed_madd))
            "#
        ),
    ),
    (
        Capability::Threads,
        wasm!(
            r#"
            (module
              (memory 1 1 shared)
              (func (result i32)
                i32.const 0
                i32.const 1
                i32.atomic.rmw.add))
            "#
        ),
    ),
    (
        Capability::StackSwitching,
        wasm!(
            r#"
            (module
              (type $task (func))
              (type $continuation (cont $task))
              (func $body)
              (elem declare func $body)
              (func (result (ref $continuation))
                ref.func $body
                cont.new $continuation))
            "#
        ),
    ),
];

/// The capabilities whose probe the browser accepts.
///
/// Each probe is a call of `WebAssembly.validate`, which accepts a module
/// only where the browser implements every feature the module uses. A
/// probe that the browser refuses, or that throws, leaves its capability
/// out: a browser without a feature loads the backend and declares less.
pub fn capabilities() -> Capabilities {
    PROBES
        .iter()
        .filter(|(_, bytes)| accepts(bytes))
        .map(|(capability, _)| *capability)
        .collect()
}

/// Whether the browser accepts the module `bytes`.
fn accepts(bytes: &[u8]) -> bool {
    WebAssembly::validate(&Uint8Array::from(bytes).into()).unwrap_or(false)
}
