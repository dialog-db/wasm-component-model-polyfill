//! The functions of JavaScript Promise Integration.

use js_sys::Function;
use wasm_bindgen::JsCast;

use crate::js;

/// The two functions of JavaScript Promise Integration (JSPI):
/// `WebAssembly.Suspending`, which makes an import that can suspend, and
/// `WebAssembly.promising`, which makes an export whose call can be
/// suspended.
///
/// The backend reads both when it is made, and keeps them for its life.
#[derive(Debug)]
pub struct Jspi {
    #[expect(
        dead_code,
        reason = "host suspension, which wraps imports with it, is not built yet"
    )]
    suspending: Function,
    #[expect(
        dead_code,
        reason = "host suspension, which wraps exports with it, is not built yet"
    )]
    promising: Function,
}

impl Jspi {
    /// The two functions, or `None` where the browser lacks either.
    pub fn read() -> Option<Self> {
        let webassembly = js::webassembly().ok()?;
        let function = |name| {
            js::get(&webassembly, name)
                .ok()?
                .dyn_into::<Function>()
                .ok()
        };
        Some(Self {
            suspending: function("Suspending")?,
            promising: function("promising")?,
        })
    }
}
