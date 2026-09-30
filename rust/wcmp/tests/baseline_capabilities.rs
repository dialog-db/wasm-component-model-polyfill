//! Baseline tests for the capabilities of the backend. The Wasm
//! features the translator validates a component with follow what the
//! backend declares, and so do the adapters it emits: over a backend
//! without exception handling, a fused adapter carries no exception
//! barrier. A component that needs a capability the backend lacks
//! fails at `Component::new` with `Unsupported` and the name of the
//! capability, before any core module compiles.

#![cfg(test)]

use wcmp::{Component, Engine, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Two inner components, where `$B` calls the function `$A` exports
/// through a fused adapter that moves only numbers.
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
          (func (export "run") (result i32)
            i32.const 20 call $double i32.const 1 i32.add))
        (core instance $i (instantiate $m
          (with "" (instance (export "double" (func $core-double))))))
        (func (export "run") (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $A))
      (instance $b (instantiate $B (with "double" (func $a "double"))))
      (export "run" (func $b "run")))
    "#
);

/// Two inner components, where `$B` passes the string `hello` to the
/// function `$A` exports, which answers its length. The fused adapter
/// copies the string from the memory of `$B` into the memory of `$A`,
/// so it imports both memories.
const TWO_MEMORY_COMPOSITION: &[u8] = component!(
    r#"
    (component
      (component $A
        (core module $m
          (memory (export "memory") 1)
          (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
            i32.const 1024)
          (func (export "length") (param i32 i32) (result i32)
            local.get 1))
        (core instance $i (instantiate $m))
        (func (export "length") (param "s" string) (result u32)
          (canon lift (core func $i "length")
            (memory (core memory $i "memory"))
            (realloc (core func $i "cabi_realloc")))))
      (component $B
        (import "length" (func $length (param "s" string) (result u32)))
        (core module $memory
          (memory (export "memory") 1)
          (data (i32.const 0) "hello"))
        (core instance $mem (instantiate $memory))
        (core func $core-length
          (canon lower (func $length) (memory (core memory $mem "memory"))))
        (core module $m
          (import "" "length" (func $length (param i32 i32) (result i32)))
          (func (export "run") (result i32)
            i32.const 0 i32.const 5 call $length))
        (core instance $i (instantiate $m
          (with "" (instance (export "length" (func $core-length))))))
        (func (export "run") (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $A))
      (instance $b (instantiate $B (with "length" (func $a "length"))))
      (export "run" (func $b "run")))
    "#
);

/// Instantiate `bytes` over `engine`, call its `run` export, and
/// answer the results.
async fn run(engine: &Engine, bytes: &[u8]) -> Vec<Val> {
    let component = Component::new(engine, bytes)
        .await
        .expect("the component translates");
    let linker: Linker<()> = Linker::new(engine);
    let mut store: Store<()> = Store::new(engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    run.call(&mut store, &[]).await.expect("call").into_vec()
}

#[wcmp_macros::test]
async fn it_runs_a_two_memory_composition_where_the_backend_declares_multi_memory() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");

    assert_eq!(run(&engine, TWO_MEMORY_COMPOSITION).await, [Val::U32(5)]);
}

/// Wasmi declares neither exception handling nor GC.
#[cfg(not(target_arch = "wasm32"))]
mod wasmi {
    use wcmp::{Component, Engine, Error, Val};
    use wcmp_macros::component;

    use super::{COMPOSITION, run};

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

    fn engine() -> Engine {
        Engine::with_backend(wcmp_wasm_core_wasmi::Wasmi::new()).expect("engine")
    }

    #[wcmp_macros::test]
    async fn it_runs_a_composition_over_a_backend_without_exceptions() {
        // An adapter with an exception barrier holds a `try_table`,
        // which Wasmi refuses to compile.
        assert_eq!(run(&engine(), COMPOSITION).await, [Val::U32(41)]);
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_component_that_needs_gc_over_a_backend_without_it() {
        let error = Component::new(&engine(), GC_COMPONENT)
            .await
            .expect_err("Wasmi has no GC");

        assert!(
            matches!(&error, Error::Unsupported { feature } if feature == "gc"),
            "{error:?}"
        );
    }
}

/// The browser, with a probe of the backend forced to fail.
#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::Cell;
    use std::rc::Rc;

    use js_sys::{Array, Function, Reflect, Uint8Array};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wcmp::{Component, Engine, Error, Val};
    use wcmp_macros::wasm;
    use wcmp_wasm_core::Capability;
    use wcmp_wasm_core::backend::Backend;
    use wcmp_wasm_core_web::Web;

    use super::{COMPOSITION, TWO_MEMORY_COMPOSITION, run};

    /// The `WebAssembly` namespace object of the page.
    fn webassembly() -> JsValue {
        Reflect::get(&js_sys::global(), &"WebAssembly".into()).expect("the page has WebAssembly")
    }

    /// The function the `WebAssembly` namespace holds as `name`.
    fn webassembly_function(name: &str) -> Function {
        Reflect::get(&webassembly(), &name.into())
            .expect("the property reads")
            .unchecked_into()
    }

    /// Put `value` in place of the property `name` of the `WebAssembly`
    /// namespace.
    fn set_webassembly_property(name: &str, value: &JsValue) {
        Reflect::set(&webassembly(), &name.into(), value).expect("the property is writable");
    }

    /// The browser backend, made while `WebAssembly.validate` refuses
    /// the probe module of `capability`, `probe`, so the backend does
    /// not declare it. Every other module validates as the browser
    /// validates it.
    fn backend_without(capability: Capability, probe: &'static [u8]) -> Web {
        let validate = webassembly_function("validate");
        let refuse = {
            let validate = validate.clone();
            Closure::<dyn Fn(JsValue) -> Result<JsValue, JsValue>>::new(move |bytes| {
                if Uint8Array::new(&bytes).to_vec() == probe {
                    return Ok(JsValue::FALSE);
                }
                Reflect::apply(&validate, &webassembly(), &Array::of1(&bytes))
            })
        };
        set_webassembly_property("validate", refuse.as_ref());
        let backend = Web::new();
        set_webassembly_property("validate", &validate);
        assert!(
            !backend.capabilities().contains(capability),
            "the backend does not declare {capability}"
        );
        backend
    }

    /// Translate `bytes` over `engine`, and count the calls of
    /// `WebAssembly.compile`, the browser's compile of a core module
    /// of a component, while it runs.
    async fn count_compiles(engine: &Engine, bytes: &[u8]) -> (Result<Component, Error>, u32) {
        let compile = webassembly_function("compile");
        let count = Rc::new(Cell::new(0));
        let counting = {
            let compile = compile.clone();
            let count = count.clone();
            Closure::<dyn Fn(JsValue) -> Result<JsValue, JsValue>>::new(move |bytes| {
                count.set(count.get() + 1);
                Reflect::apply(&compile, &webassembly(), &Array::of1(&bytes))
            })
        };
        set_webassembly_property("compile", counting.as_ref());
        let component = Component::new(engine, bytes).await;
        set_webassembly_property("compile", &compile);
        (component, count.get())
    }

    #[wcmp_macros::test]
    async fn it_refuses_a_two_memory_adapter_before_any_module_compiles_without_multi_memory() {
        let backend = backend_without(
            Capability::MultiMemory,
            wasm!(r#"(module (memory 1) (memory 1))"#),
        );
        let engine = Engine::with_backend(backend).expect("engine");

        let (component, compiles) = count_compiles(&engine, TWO_MEMORY_COMPOSITION).await;

        let error = component.expect_err("the adapter needs two memories");
        assert!(
            matches!(&error, Error::Unsupported { feature } if feature == "multi_memory"),
            "{error:?}"
        );
        assert_eq!(compiles, 0, "no core module compiles");
    }

    #[wcmp_macros::test]
    async fn it_compiles_the_modules_of_a_two_memory_composition_with_multi_memory() {
        // The control of the count above: the same composition, over a
        // browser that declares multi-memory, compiles its modules
        // through the counted function.
        let engine = Engine::with_backend(Web::new()).expect("engine");

        let (component, compiles) = count_compiles(&engine, TWO_MEMORY_COMPOSITION).await;

        component.expect("the component translates");
        assert!(compiles > 0, "the core modules compile");
    }

    #[wcmp_macros::test]
    async fn it_runs_a_composition_without_exceptions() {
        let backend = backend_without(
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
        );
        let engine = Engine::with_backend(backend).expect("engine");

        assert_eq!(run(&engine, COMPOSITION).await, [Val::U32(41)]);
    }
}

#[path = "support/backend.rs"]
mod test_backend;
