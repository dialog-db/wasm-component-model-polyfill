//! The functions of JavaScript Promise Integration.

use js_sys::{Array, Function, Reflect};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wcmp_wasm_core::Result;

use crate::errors;
use crate::js;

#[wasm_bindgen]
extern "C" {
    /// A `WebAssembly.Suspending` object: an import that can suspend.
    #[wasm_bindgen(js_namespace = WebAssembly)]
    type Suspending;

    /// `new WebAssembly.Suspending(function)`.
    #[wasm_bindgen(constructor, js_namespace = WebAssembly, catch)]
    fn new(function: &Function) -> core::result::Result<Suspending, JsValue>;
}

/// The two functions of JavaScript Promise Integration (JSPI):
/// `WebAssembly.Suspending`, which makes an import that can suspend, and
/// `WebAssembly.promising`, which makes an export whose call can be
/// suspended.
///
/// The backend reads both when it is made, and keeps `promising` for its
/// life. It makes each suspending import with a typed constructor, since
/// it names no generic way to construct an object from a function.
#[derive(Debug)]
pub struct Jspi {
    promising: Function,
}

impl Jspi {
    /// The two functions of the page's `WebAssembly` namespace, or `None`
    /// where the browser lacks either.
    pub fn read() -> Option<Self> {
        let webassembly = js::webassembly().ok()?;
        let function = |name| {
            js::get(&webassembly, name)
                .ok()?
                .dyn_into::<Function>()
                .ok()
        };
        function("Suspending")?;
        Some(Self {
            promising: function("promising")?,
        })
    }

    /// The import `new WebAssembly.Suspending(function)`.
    ///
    /// A guest that calls the import from inside a stack that a promising
    /// call began, with only WebAssembly frames in between, suspends that
    /// stack on the promise `function` answers. The stack resumes on a
    /// microtask once the promise resolves.
    pub fn suspending(&self, function: &Function) -> Result<JsValue> {
        Suspending::new(function)
            .map(JsValue::from)
            .map_err(|error| errors::backend(errors::message(&error)))
    }

    /// The function `WebAssembly.promising(function)`, whose call runs
    /// `function` on a stack of its own and answers a promise of its
    /// results.
    pub fn promising(&self, function: &Function) -> Result<Function> {
        Reflect::apply(&self.promising, &JsValue::UNDEFINED, &Array::of1(function))
            .and_then(|promising| promising.dyn_into::<Function>())
            .map_err(|error| errors::backend(errors::message(&error)))
    }
}
