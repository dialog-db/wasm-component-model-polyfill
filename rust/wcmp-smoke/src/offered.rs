// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Which suspend provider this target offers, worked out without the
//! engine, so the introspection story can check the engine's answer
//! against it.
//!
//! Natively the answer follows from the platform: Wasmtime implements
//! the WebAssembly stack-switching instructions on x86_64 Linux only,
//! so the engine selects the stack-switching provider there and no
//! provider anywhere else.
//!
//! In the browser the answer follows from the page's `WebAssembly`
//! namespace: a browser that ships JavaScript Promise Integration has
//! `Suspending` and `promising` on it as functions, and one that does
//! not, such as Safari before 27, has neither. The flake's Chromium
//! ships it, and a person who opens the page in an older browser sees
//! the engine answer that it has no provider, and the stories that
//! need a suspension report the outcome documented for that case.

use wcmp::SuspendProviderKind;

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
mod imp {
    use super::SuspendProviderKind;

    /// The provider this target offers an engine that is allowed one.
    pub fn offered() -> SuspendProviderKind {
        SuspendProviderKind::StackSwitching
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::SuspendProviderKind;
    use wasm_bindgen::{JsCast, JsValue};

    /// The provider this target offers an engine that is allowed one.
    pub fn offered() -> SuspendProviderKind {
        if has_function("Suspending") && has_function("promising") {
            SuspendProviderKind::HostSuspension
        } else {
            SuspendProviderKind::None
        }
    }

    /// Whether the global `WebAssembly` namespace has a function
    /// named `name`.
    fn has_function(name: &str) -> bool {
        let Ok(namespace) =
            js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("WebAssembly"))
        else {
            return false;
        };
        js_sys::Reflect::get(&namespace, &JsValue::from_str(name))
            .is_ok_and(|entry| entry.is_instance_of::<js_sys::Function>())
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(target_arch = "x86_64", target_os = "linux")
)))]
mod imp {
    use super::SuspendProviderKind;

    /// The provider this target offers an engine that is allowed one.
    pub fn offered() -> SuspendProviderKind {
        SuspendProviderKind::None
    }
}

pub use imp::offered;
