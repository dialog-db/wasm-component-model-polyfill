// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The modules a host compiled ahead of time, and the record of every
//! module the backend makes.

use std::cell::RefCell;
use std::collections::HashMap;

use js_sys::{Object, Uint8Array, WebAssembly};
use sha2::{Digest, Sha256};
use wasm_bindgen::{JsCast, JsValue};

/// The modules the backend takes instead of compiling, by the hash of
/// their bytes, and the record of the bytes of every module it makes.
///
/// Some hosts compile no WebAssembly at run time: a Cloudflare Worker
/// refuses `WebAssembly.compile` and `new WebAssembly.Module` alike, and
/// takes only the modules it was deployed with. The backend makes modules
/// of two kinds: the core modules of a component, which the host hands it,
/// and the small modules it generates as it links (accessors, bridges,
/// carriers, wrappers, entrances and the dispatcher). Every one of them
/// goes through here, so a host that cannot compile hands the backend each
/// module it will make, compiled ahead of time, with [`provide`]. The key
/// is the SHA-256 of the module's bytes, in lowercase hexadecimal, as
/// [`key`] spells it.
///
/// To learn which modules those are, a host runs the same component where
/// compiling is allowed, or where it is not, with [`record`] on. Every
/// module the backend makes from then on, given or compiled, is kept with
/// its key, and [`recorded`] hands them out. A module the backend makes is
/// a function of the component and of the capabilities the backend
/// declares, so one record serves every run of the same component on a
/// host that declares the same capabilities.
///
/// A module that was not provided compiles, as without this type. Where
/// the host refuses to compile, the error names the key of the module that
/// was missing.
///
/// The modules are kept for the thread, as the backend's dispatcher is.
///
/// [`provide`]: Precompiled::provide
/// [`key`]: Precompiled::key
/// [`record`]: Precompiled::record
/// [`recorded`]: Precompiled::recorded
#[derive(Debug)]
pub struct Precompiled;

/// What the thread was provided and what it recorded.
#[derive(Default)]
struct Modules {
    provided: HashMap<String, WebAssembly::Module>,
    recorded: Option<Vec<(String, Vec<u8>)>>,
}

thread_local! {
    static MODULES: RefCell<Modules> = RefCell::default();
}

impl Precompiled {
    /// The key of the module `bytes`: the SHA-256 of the bytes, in
    /// lowercase hexadecimal.
    pub fn key(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Serve `module` for every module whose bytes have the key `key`.
    pub fn provide(key: impl Into<String>, module: WebAssembly::Module) {
        MODULES.with(|modules| {
            modules.borrow_mut().provided.insert(key.into(), module);
        });
    }

    /// Keep the bytes of every module the backend makes from now on.
    pub fn record() {
        MODULES.with(|modules| {
            modules.borrow_mut().recorded.get_or_insert_with(Vec::new);
        });
    }

    /// The modules recorded since [`record`](Self::record), each once,
    /// as an object from each key to the module's bytes, a `Uint8Array`.
    /// Recording goes on.
    pub fn recorded() -> Object {
        let recorded = Object::new();
        MODULES.with(|modules| {
            for (key, bytes) in modules.borrow().recorded.iter().flatten() {
                // Setting a property of a plain object cannot fail.
                let _ = js_sys::Reflect::set(
                    &recorded,
                    &JsValue::from_str(key),
                    &Uint8Array::from(&bytes[..]),
                );
            }
        });
        recorded
    }
}

/// What the backend knows of the module `bytes` before it compiles: the
/// module it was provided, or none, having recorded the bytes. Hashes the
/// bytes only where something was provided or is recorded.
fn known(bytes: &[u8]) -> Result<WebAssembly::Module, Option<String>> {
    MODULES.with(|modules| {
        let mut modules = modules.borrow_mut();
        if modules.provided.is_empty() && modules.recorded.is_none() {
            return Err(None);
        }
        let key = Precompiled::key(bytes);
        if let Some(recorded) = modules.recorded.as_mut()
            && !recorded.iter().any(|(seen, _)| *seen == key)
        {
            recorded.push((key.clone(), bytes.to_vec()));
        }
        modules.provided.get(&key).cloned().ok_or(Some(key))
    })
}

/// `error`, the host's refusal to compile, with the key of the module it
/// refused where there is one.
fn missing(error: JsValue, key: Option<String>) -> JsValue {
    let Some(key) = key else {
        return error;
    };
    let message = error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .unwrap_or_else(|| format!("{error:?}"));
    js_sys::Error::new(&format!("{message} (module {key} was not precompiled)")).into()
}

/// The module `bytes`: the one provided for its key, or else compiled
/// synchronously.
pub fn module(bytes: &[u8]) -> Result<WebAssembly::Module, JsValue> {
    match known(bytes) {
        Ok(module) => Ok(module),
        Err(key) => WebAssembly::Module::new(&Uint8Array::from(bytes).into())
            .map_err(|error| missing(error, key)),
    }
}

/// The module `bytes`: the one provided for its key, or else compiled
/// with `WebAssembly.compile`, which loads a module of any size.
pub async fn compile(bytes: &[u8]) -> Result<WebAssembly::Module, JsValue> {
    let key = match known(bytes) {
        Ok(module) => return Ok(module),
        Err(key) => key,
    };
    WebAssembly::compile(&Uint8Array::from(bytes).into())
        .await
        .map_err(|error| missing(error, key))?
        .dyn_into::<WebAssembly::Module>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module with one function, `f`, that returns `value`: distinct
    /// bytes for each `value`.
    fn bytes(value: u8) -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic, version
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type: () -> i32
            0x03, 0x02, 0x01, 0x00, // function 0 of type 0
            0x07, 0x05, 0x01, 0x01, b'f', 0x00, 0x00, // export "f"
            0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, value, 0x0b, // i32.const value
        ]
    }

    /// The bytes `recorded` holds for `key`.
    fn recorded_bytes(recorded: &Object, key: &str) -> Option<Vec<u8>> {
        js_sys::Reflect::get(recorded, &JsValue::from_str(key))
            .ok()
            .and_then(|value| value.dyn_into::<Uint8Array>().ok())
            .map(|bytes| bytes.to_vec())
    }

    #[wcmp_macros::test]
    fn it_keys_a_module_by_the_sha256_of_its_bytes() {
        assert_eq!(
            Precompiled::key(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[wcmp_macros::test]
    fn it_serves_the_module_provided_for_the_key_of_its_bytes() {
        let bytes = bytes(41);
        let provided = WebAssembly::Module::new(&Uint8Array::from(&bytes[..]).into())
            .expect("the module compiles");
        Precompiled::provide(Precompiled::key(&bytes), provided.clone());
        let served = module(&bytes).expect("the module is served");
        assert!(Object::is(&served, &provided), "the provided module itself");
    }

    #[wcmp_macros::test]
    fn it_compiles_a_module_it_was_not_given() {
        let bytes = bytes(42);
        let first = module(&bytes).expect("the module compiles");
        let second = module(&bytes).expect("the module compiles");
        assert!(!Object::is(&first, &second), "each compile makes its own");
    }

    #[wcmp_macros::test]
    async fn it_records_each_module_it_makes_once_by_its_key() {
        Precompiled::record();
        let generated = bytes(43);
        let loaded = bytes(44);
        module(&generated).expect("the module compiles");
        module(&generated).expect("the module compiles");
        compile(&loaded).await.expect("the module compiles");
        let recorded = Precompiled::recorded();
        assert_eq!(
            recorded_bytes(&recorded, &Precompiled::key(&generated)),
            Some(generated)
        );
        assert_eq!(
            recorded_bytes(&recorded, &Precompiled::key(&loaded)),
            Some(loaded)
        );
    }
}
