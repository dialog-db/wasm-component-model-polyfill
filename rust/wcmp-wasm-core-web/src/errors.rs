// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The errors of the JavaScript API, as the runtime layer reports them.

use js_sys::WebAssembly;
use wasm_bindgen::{JsCast, JsValue};
use wcmp_wasm_core::{Error, ImportType, TrapKind};

use crate::carrier::Carrier;
use crate::traps;

/// The message of the JavaScript error `error`, or the value in words where
/// it is not an error.
pub fn message(error: &JsValue) -> String {
    if let Some(error) = error.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

/// [`Error::Compile`] for a compile that the engine refused, with the
/// engine's message.
///
/// A synchronous compile of a module above the browser's limit fails here
/// too: the browser throws a `RangeError` whose message names the limit.
pub fn compile(error: &JsValue) -> Error {
    Error::Compile {
        message: message(error),
    }
}

/// The error of an instantiation that the engine refused, where `failed`
/// is the import it failed on, as the backend tells it without the
/// engine's words.
///
/// An instantiation that failed on an import is [`Error::Link`], which
/// names it with the engine's message. Every other `LinkError` is
/// [`Error::Link`] too, with no names, since the backend cannot tell which
/// import it is about, or whether it is about one at all. A trap in the
/// start function is [`Error::Trap`], as [`guest`] reads it.
pub fn instantiate(
    error: &JsValue,
    failed: Option<&ImportType>,
    carrier: &mut Carrier,
    shared_memory: bool,
) -> Error {
    if let Some(error) = link(error, failed) {
        return error;
    }
    if error.is_instance_of::<WebAssembly::CompileError>() {
        return Error::Compile {
            message: message(error),
        };
    }
    guest(error, carrier, shared_memory)
}

/// [`Error::Link`] for an instantiation that failed with `error` on the
/// import `failed`, or with a `LinkError` on no import the backend can
/// name. See [`instantiate`].
fn link(error: &JsValue, failed: Option<&ImportType>) -> Option<Error> {
    let (module, name) = match failed {
        Some(import) => (import.module().to_string(), import.name().to_string()),
        None if error.is_instance_of::<WebAssembly::LinkError>() => (String::new(), String::new()),
        None => return None,
    };
    Some(Error::Link {
        module,
        name,
        message: message(error),
    })
}

/// The error of a call into a guest of a store that threw `error`, where
/// no host function of the store failed.
///
/// An exception that no guest caught is [`TrapKind::UncaughtException`],
/// with the exception rooted in the store's table of exceptions through
/// `carrier`. Anything else is as [`call`] reads it, where `shared_memory`
/// tells whether any memory of the store is shared.
pub fn guest(error: &JsValue, carrier: &mut Carrier, shared_memory: bool) -> Error {
    if error.is_instance_of::<WebAssembly::Exception>() {
        return Error::Trap(match carrier.root(error) {
            Ok(exception) => TrapKind::UncaughtException(exception),
            Err(error) => TrapKind::Other(error.to_string()),
        });
    }
    read(error, shared_memory)
}

/// The error of a step over the JavaScript API that threw `error`.
///
/// A `TypeError` is a value that the JavaScript API cannot carry to or from
/// the guest: [`Error::TypeMismatch`]. A trap is a `RuntimeError`, the
/// `RangeError` of an exhausted stack, or an error whose message an
/// engine's table knows, such as the `InternalError` of SpiderMonkey's
/// exhausted stack. Its kind is the one the table gives the engine's
/// message, and never a wrong one. An exception that no guest caught is a
/// trap with the engine's words for it: only [`guest`] roots it.
pub fn call(error: &JsValue) -> Error {
    if error.is_instance_of::<WebAssembly::Exception>() {
        return Error::Trap(TrapKind::Other(message(error)));
    }
    read(error, true)
}

/// The error that `error`, which is not an exception, stands for, where
/// `shared_memory` tells whether any memory of the store is shared. See
/// [`call`].
fn read(error: &JsValue, shared_memory: bool) -> Error {
    let text = message(error);
    if error.is_instance_of::<js_sys::TypeError>() {
        return Error::TypeMismatch { message: text };
    }
    let kind = traps::kind(&text, shared_memory);
    if error.is_instance_of::<WebAssembly::RuntimeError>()
        || error.is_instance_of::<js_sys::RangeError>()
        || !matches!(kind, TrapKind::Other(_))
    {
        return Error::Trap(kind);
    }
    Error::Backend { message: text }
}

/// [`Error::Backend`] with `message`.
pub fn backend(message: impl Into<String>) -> Error {
    Error::Backend {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use wcmp_wasm_core::{ExternType, FuncType, ValType};

    use super::*;

    #[wcmp_macros::test]
    fn it_reads_every_link_error_as_a_link_error_whatever_its_words() {
        let import = ImportType::new(
            "host",
            "notify",
            ExternType::Func(FuncType::new([ValType::I32], [])),
        );
        // The words of V8, JavaScriptCore, and SpiderMonkey, and none.
        for text in [
            "WebAssembly.instantiate(): Import #0 \"host\" \"notify\": imported function does \
             not match the expected type",
            "imported function host:notify signature doesn't match the provided WebAssembly \
             function's signature",
            "imported global type mismatch",
            "",
        ] {
            let error = WebAssembly::LinkError::new(text);
            assert!(
                matches!(
                    link(&error, Some(&import)),
                    Some(Error::Link { ref module, ref name, .. })
                        if module == "host" && name == "notify"
                ),
                "{text:?}"
            );
            assert!(
                matches!(
                    link(&error, None),
                    Some(Error::Link { ref module, ref name, .. })
                        if module.is_empty() && name.is_empty()
                ),
                "{text:?}"
            );
        }

        // A `TypeError` is about an import only where the backend found
        // the one it failed on.
        let error = js_sys::TypeError::new("import host:notify must be an object");
        assert!(matches!(
            link(&error, Some(&import)),
            Some(Error::Link { .. })
        ));
        assert!(link(&error, None).is_none());
    }

    #[wcmp_macros::test]
    fn it_reads_the_exhausted_stack_of_spidermonkey_as_a_stack_overflow() {
        // SpiderMonkey throws an `InternalError`, which no other engine
        // has, so the test builds one: an `Error` with that name, which is
        // neither a `RuntimeError` nor a `RangeError`.
        let error = js_sys::Error::new("too much recursion");
        error.set_name("InternalError");
        for shared_memory in [false, true] {
            assert!(matches!(
                read(&error, shared_memory),
                Error::Trap(TrapKind::StackOverflow)
            ));
        }
    }

    #[wcmp_macros::test]
    fn it_never_reads_a_type_error_as_a_trap() {
        for text in [
            "too much recursion",
            "unreachable",
            "Maximum call stack size exceeded",
            "a value the JavaScript API cannot carry",
        ] {
            let error = js_sys::TypeError::new(text);
            assert!(
                matches!(
                    read(&error, false),
                    Error::TypeMismatch { ref message } if message == text
                ),
                "{text:?}"
            );
        }
    }
}
