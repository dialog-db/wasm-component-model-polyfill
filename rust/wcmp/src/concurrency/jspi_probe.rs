//! The probe that asks the browser whether it offers JavaScript
//! Promise Integration.

#[cfg(target_arch = "wasm32")]
use js_sys::{Function, Reflect};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

/// The probe that asks the browser whether it offers JavaScript
/// Promise Integration, the mechanism of the JSPI provider.
///
/// The probe passes when the global `WebAssembly` namespace has
/// `Suspending` and `promising`, and both are functions. It runs in
/// the browser only: on a native target it never passes. It reads
/// two properties and calls nothing, so it is synchronous, and an
/// engine runs it while it is constructed, after the switch probe.
///
/// The probe asks whether the API exists, not whether it works. The
/// switch probe runs a thread through one suspension, because an
/// engine can validate the stack-switching instructions and still not
/// run them. A browser that defines the JSPI API implements it, and
/// running a promising call takes a microtask, which a synchronous
/// probe cannot wait for.
#[derive(Clone, Copy, Debug, Default)]
pub struct JspiProbe;

impl JspiProbe {
    /// The probe over the global `WebAssembly` namespace.
    pub fn new() -> Self {
        Self
    }

    /// Whether the browser offers JavaScript Promise Integration.
    #[cfg(target_arch = "wasm32")]
    pub fn passes(self) -> bool {
        Reflect::get(&js_sys::global(), &"WebAssembly".into())
            .is_ok_and(|namespace| Self::offered_by(&namespace))
    }

    /// Whether the browser offers JavaScript Promise Integration,
    /// which no native target does.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn passes(self) -> bool {
        false
    }

    /// Whether `namespace`, which stands for the `WebAssembly`
    /// namespace, has `Suspending` and `promising` as functions. A
    /// test uses it to prove the probe's failure paths.
    #[cfg(target_arch = "wasm32")]
    pub fn offered_by(namespace: &JsValue) -> bool {
        ["Suspending", "promising"].into_iter().all(|name| {
            Reflect::get(namespace, &name.into())
                .is_ok_and(|member| member.is_instance_of::<Function>())
        })
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use js_sys::Object;

    use super::*;

    /// A stand-in for the `WebAssembly` namespace with the members
    /// `functions` set to a function and `numbers` set to a number.
    fn namespace(functions: &[&str], numbers: &[&str]) -> JsValue {
        let namespace = Object::new();
        let function: Function = namespace.constructor();
        for name in functions {
            Reflect::set(&namespace, &(*name).into(), &function).expect("set");
        }
        for name in numbers {
            Reflect::set(&namespace, &(*name).into(), &1.into()).expect("set");
        }
        namespace.into()
    }

    #[wcmp_macros::test]
    fn it_passes_where_both_members_are_functions() {
        assert!(JspiProbe::offered_by(&namespace(
            &["Suspending", "promising"],
            &[]
        )));
    }

    #[wcmp_macros::test]
    fn it_fails_where_a_member_is_missing_or_not_a_function() {
        assert!(!JspiProbe::offered_by(&namespace(&["promising"], &[])));
        assert!(!JspiProbe::offered_by(&namespace(&["Suspending"], &[])));
        assert!(!JspiProbe::offered_by(&namespace(
            &["Suspending"],
            &["promising"]
        )));
        assert!(!JspiProbe::offered_by(&JsValue::UNDEFINED));
    }
}
