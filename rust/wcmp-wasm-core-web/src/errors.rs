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

/// The error of an instantiation of a module with the imports `imports`
/// that the engine refused.
///
/// The engine names an import that does not link by its place, as
/// `Import #3`, in a `LinkError`, or in a `TypeError` where the imports
/// object lacks it. The backend reads the place back to name the import in
/// [`Error::Link`]. A trap in the start function is [`Error::Trap`], as
/// [`guest`] reads it.
pub fn instantiate(
    error: &JsValue,
    imports: &[ImportType],
    carrier: &mut Carrier,
    shared_memory: bool,
) -> Error {
    let text = message(error);
    let linking = error.is_instance_of::<WebAssembly::LinkError>()
        || error.is_instance_of::<js_sys::TypeError>();
    if linking && let Some(import) = import_index(&text).and_then(|index| imports.get(index)) {
        return Error::Link {
            module: import.module().to_string(),
            name: import.name().to_string(),
            message: text,
        };
    }
    if error.is_instance_of::<WebAssembly::CompileError>() {
        return Error::Compile { message: text };
    }
    guest(error, carrier, shared_memory)
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

/// The place of the import that the engine's message `text` names, as
/// `Import #3`.
fn import_index(text: &str) -> Option<usize> {
    let (_, after) = text.split_once("Import #")?;
    let digits = after
        .find(|c: char| !c.is_ascii_digit())
        .map_or(after, |end| &after[..end]);
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_reads_the_place_of_an_import_from_the_engines_message() {
        assert_eq!(
            import_index(
                "WebAssembly.instantiate(): Import #12 \"host\" \"notify\": \
                 function import requires a callable"
            ),
            Some(12)
        );
        assert_eq!(import_index("Import #3"), Some(3));
        assert_eq!(import_index("no place here"), None);
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
