// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Small steps over the JavaScript API, each of which catches what
//! JavaScript throws.
//!
//! A JavaScript exception that crosses a Rust frame aborts the program. So
//! the backend reaches every method or property that can throw through
//! `Reflect`, which hands the exception back as an `Err`. None of these
//! steps makes a function from a string of source.

use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};

/// The property `key` of `target`.
pub fn get(target: &JsValue, key: &str) -> Result<JsValue, JsValue> {
    Reflect::get(target, &JsValue::from_str(key))
}

/// Sets the property `key` of `target` to `value`.
pub fn set(target: &JsValue, key: &str, value: &JsValue) -> Result<(), JsValue> {
    Reflect::set(target, &JsValue::from_str(key), value).map(|_| ())
}

thread_local! {
    /// The key `value`, made once. A key made from a Rust string crosses
    /// into JavaScript and decodes its bytes each time, and the host reads
    /// or writes the value of a global on every call of a guest.
    static VALUE: JsValue = JsValue::from_str("value");
}

/// The property `value` of `target`: the value of a global.
pub fn get_value(target: &JsValue) -> Result<JsValue, JsValue> {
    VALUE.with(|key| Reflect::get(target, key))
}

/// Sets the property `value` of `target`, the value of a global, to
/// `value`.
pub fn set_value(target: &JsValue, value: &JsValue) -> Result<(), JsValue> {
    VALUE
        .with(|key| Reflect::set(target, key, value))
        .map(|_| ())
}

/// Calls the method `name` of `target` with `args`.
pub fn call_method(target: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let method = get(target, name)?.dyn_into::<Function>()?;
    Reflect::apply(&method, target, &args.iter().collect::<Array>())
}

/// The global `WebAssembly` namespace object.
pub fn webassembly() -> Result<Object, JsValue> {
    get(&js_sys::global(), "WebAssembly")?.dyn_into::<Object>()
}

/// A plain object with the properties `entries`.
pub fn object(entries: &[(&str, JsValue)]) -> Result<Object, JsValue> {
    let object = Object::new();
    for (key, value) in entries {
        set(&object, key, value)?;
    }
    Ok(object)
}

/// A number of the JavaScript API that counts pages, elements, or bytes: a
/// `Number` for a memory or a table addressed with 32-bit numbers, and a
/// `BigInt` for one addressed with 64-bit numbers.
pub fn address(value: u64, is_64: bool) -> JsValue {
    if is_64 {
        JsValue::from(value)
    } else {
        JsValue::from_f64(value as f64)
    }
}

/// The count `value` that the JavaScript API gave, as a `Number` or a
/// `BigInt`.
pub fn count(value: &JsValue) -> Option<u64> {
    if let Some(number) = value.as_f64() {
        return (number >= 0.0 && number.fract() == 0.0).then_some(number as u64);
    }
    u64::try_from(value.clone()).ok()
}
