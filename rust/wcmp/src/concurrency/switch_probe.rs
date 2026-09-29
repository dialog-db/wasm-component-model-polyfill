//! The probe that asks an engine whether it switches stacks with the
//! instructions of the WebAssembly stack-switching proposal.

use crate::runtime_layer::{
    Backend, Engine as RuntimeEngine, Imports, Instance as RuntimeInstance,
    Module as RuntimeModule, Store as RuntimeStore, Val as RuntimeVal,
};

/// The probe that asks an engine whether it switches stacks with the
/// instructions of the WebAssembly stack-switching proposal.
///
/// The probe compiles and instantiates a small core module and calls
/// its one export, `run`. `run` starts one thread with `cont.new` and
/// `resume`, and the thread suspends on a control tag at once. `run`
/// resumes it, and the thread returns. `run` answers `1` when the
/// thread suspended once and then finished, `0` when the first
/// resume finished without a suspension, and `2` when the thread
/// suspended again instead of finishing. The probe passes on `1`
/// alone. An engine that rejects the module, or that runs it with any
/// other outcome, a trap included, fails the probe. The probe proves
/// that the feature works, not only that the engine validates it.
///
/// The module is [`MODULE`](Self::MODULE), 97 bytes the polyfill
/// carries as a constant in its own binary, so the probe never
/// fetches anything. Its text is:
///
/// ```text
/// (module
///   (type (func))                    ;; 0: the tag's and the thread's type
///   (type (cont 0))                  ;; 1: a thread
///   (tag (type 0))
///   (func (type 0)                   ;; the thread: suspend, then return
///     suspend 0)
///   (elem declare func 0)
///   (func (export "run") (result i32)
///     (local (ref null 1))
///     block (result (ref 1))
///       ref.func 0
///       cont.new 1
///       resume 1 (on 0 0)            ;; start the thread
///       i32.const 0                  ;; it finished without suspending
///       return
///     end
///     local.set 0                    ;; it suspended: keep the rest
///     block (result (ref 1))
///       local.get 0
///       resume 1 (on 0 0)            ;; resume it
///       i32.const 1                  ;; it finished: the probe passes
///       return
///     end
///     drop
///     i32.const 2))                  ;; it suspended again
/// ```
///
/// The probe is small and synchronous, so an engine can run it while
/// it is constructed.
#[derive(Clone, Copy, Debug)]
pub struct SwitchProbe {
    module: &'static [u8],
}

impl SwitchProbe {
    /// The bytes of the probe module the polyfill carries.
    pub const MODULE: &'static [u8] = &[
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // header
        0x01, 0x0a, 0x03, 0x60, 0x00, 0x00, 0x5d, 0x00, // types
        0x60, 0x00, 0x01, 0x7f, //
        0x03, 0x03, 0x02, 0x00, 0x02, // functions
        0x0d, 0x03, 0x01, 0x00, 0x00, // tags
        0x07, 0x07, 0x01, 0x03, 0x72, 0x75, 0x6e, 0x00, 0x01, // exports
        0x09, 0x05, 0x01, 0x03, 0x00, 0x01, 0x00, // elements
        0x0a, 0x31, 0x02, // code
        0x04, 0x00, 0xe2, 0x00, 0x0b, // the thread
        0x2a, 0x01, 0x01, 0x63, 0x01, // `run` and its local
        0x02, 0x64, 0x01, 0xd2, 0x00, 0xe0, 0x01, 0xe3, 0x01, 0x01, 0x00, 0x00, 0x00, //
        0x41, 0x00, 0x0f, 0x0b, 0x21, 0x00, //
        0x02, 0x64, 0x01, 0x20, 0x00, 0xe3, 0x01, 0x01, 0x00, 0x00, 0x00, //
        0x41, 0x01, 0x0f, 0x0b, 0x1a, 0x41, 0x02, 0x0b,
    ];

    /// What `run` answers when the thread suspended once and then
    /// finished.
    const PASSED: i32 = 1;

    /// The probe the polyfill runs, over [`MODULE`](Self::MODULE).
    pub fn new() -> Self {
        Self::over(Self::MODULE)
    }

    /// A probe over another module with the same export, which a
    /// test uses to prove the probe's failure path.
    pub fn over(module: &'static [u8]) -> Self {
        Self { module }
    }

    /// Whether `engine` runs the probe's thread to the end through
    /// one suspension. Every failure answers `false`: a module the
    /// engine rejects, an instantiation or a call that fails, and any
    /// answer but the one that passes.
    pub fn passes(self, engine: &RuntimeEngine<Backend>) -> bool {
        let Ok(module) = RuntimeModule::new(engine, self.module) else {
            return false;
        };
        let mut store = RuntimeStore::new(engine, ());
        let Ok(instance) = RuntimeInstance::new(&mut store, &module, &Imports::default()) else {
            return false;
        };
        let Some(run) = instance
            .get_export(&store, "run")
            .and_then(|export| export.into_func())
        else {
            return false;
        };
        let mut answer = [RuntimeVal::I32(0)];
        run.call(&mut store, &[], &mut answer).is_ok()
            && matches!(answer, [RuntimeVal::I32(Self::PASSED)])
    }
}

impl Default for SwitchProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_carries_the_module_its_text_spells() {
        let assembled = wcmp_macros::wasm!(
            r#"
            (module
              (type (func))
              (type (cont 0))
              (tag (type 0))
              (func (type 0)
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

        assert_eq!(SwitchProbe::MODULE, assembled);
    }
}
