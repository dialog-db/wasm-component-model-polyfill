//! The errors of the JavaScript API, as the runtime layer reports them.

use js_sys::WebAssembly;
use wasm_bindgen::{JsCast, JsValue};
use wcmp_wasm_core::{Error, ImportType, TrapKind};

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
/// [`Error::Link`]. A trap in the start function is [`Error::Trap`].
pub fn instantiate(error: &JsValue, imports: &[ImportType]) -> Error {
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
    call(error)
}

/// The error of a call into a guest that threw `error`.
///
/// A `TypeError` is a value that the JavaScript API cannot carry to or from
/// the guest: [`Error::TypeMismatch`]. A `RuntimeError`, the `RangeError`
/// of an exhausted stack, and an exception that no guest caught are a
/// trap, with the engine's message.
pub fn call(error: &JsValue) -> Error {
    let text = message(error);
    if error.is_instance_of::<js_sys::TypeError>() {
        return Error::TypeMismatch { message: text };
    }
    if error.is_instance_of::<WebAssembly::RuntimeError>()
        || error.is_instance_of::<js_sys::RangeError>()
        || error.is_instance_of::<WebAssembly::Exception>()
    {
        return Error::Trap(TrapKind::Other(text));
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
}
