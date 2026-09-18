//! Procedural macros for the Wasm Component Model Polyfill workspace.
//!
//! Three macros live here:
//!
//! - [`macro@test`] — cross-target attribute that expands to `#[tokio::test]`
//!   on native, or to the built-in `#[test]` for a synchronous body, and to
//!   `#[wasm_bindgen_test]` on `wasm32-unknown-unknown`.
//! - [`wasm!`] — assemble inline WebAssembly Text Format into a core-module
//!   byte slice at compile time.
//! - [`component!`] — assemble inline WebAssembly Text Format into a
//!   component-format byte slice at compile time.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{ItemFn, LitStr, parse_macro_input};

/// Assemble inline WebAssembly Text Format into a core-module binary.
///
/// The macro accepts a single string literal containing WAT and returns a
/// `&'static [u8]` of the assembled bytes. Assembly happens at compile time;
/// any parse error is surfaced as a `compile_error!` on the macro call site.
///
/// The top-level form must be a core module (`(module ...)`); a `(component
/// ...)` form is rejected with a clear error pointing at [`component!`].
///
/// # Example
///
/// ```ignore
/// const ADDER: &[u8] = wcmp_macros::wasm!(r#"
///     (module
///       (func (export "add") (param i32 i32) (result i32)
///         local.get 0 local.get 1 i32.add))
/// "#);
/// ```
/// Cross-target test attribute.
///
/// The attribute marks a test as one that runs on both targets. It takes an
/// `async fn` or a plain `fn`.
///
/// An `async fn` expands to `#[tokio::test]` on
/// `cfg(not(target_arch = "wasm32"))`. A synchronous `fn` expands to the
/// language's built-in `#[test]` there. Both expand to `#[wasm_bindgen_test]`
/// on `cfg(target_arch = "wasm32")`, because the browser runner collects only
/// `wasm_bindgen_test` functions and a plain `#[test]` would therefore be a
/// native-only test.
///
/// The macro takes no arguments. Callers must have `tokio` in scope on native
/// targets when the body is `async`, and `wasm_bindgen_test` in scope on
/// `wasm32-unknown-unknown`.
///
/// # Example
///
/// ```ignore
/// #[wcmp_macros::test]
/// async fn it_works() {
///     assert_eq!(2 + 2, 4);
/// }
///
/// #[wcmp_macros::test]
/// fn it_works_without_awaiting_anything() {
///     assert_eq!(2 + 2, 4);
/// }
/// ```
#[proc_macro_attribute]
pub fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        let span = TokenStream2::from(attr)
            .into_iter()
            .next()
            .map_or_else(Span::call_site, |t| t.span());
        return error(
            span,
            "`#[wcmp_macros::test]` takes no arguments".to_string(),
        )
        .into();
    }

    let function = parse_macro_input!(item as ItemFn);

    // The two native halves differ: an `async fn` needs a runtime to be
    // driven on, and a plain `fn` is what the built-in attribute already
    // takes. The browser half is `wasm_bindgen_test` either way, since its
    // runner takes both shapes.
    let native = if function.sig.asyncness.is_some() {
        quote!(::tokio::test)
    } else {
        quote!(test)
    };

    quote! {
        #[cfg_attr(not(target_arch = "wasm32"), #native)]
        #[cfg_attr(target_arch = "wasm32", ::wasm_bindgen_test::wasm_bindgen_test)]
        #function
    }
    .into()
}

#[proc_macro]
pub fn wasm(input: TokenStream) -> TokenStream {
    let literal = parse_macro_input!(input as LitStr);
    assemble(literal, BinaryKind::Module).into()
}

/// Assemble inline WebAssembly Text Format into a component binary.
///
/// The macro accepts a single string literal containing WAT and returns a
/// `&'static [u8]` of the assembled bytes. Assembly happens at compile time;
/// any parse error is surfaced as a `compile_error!` on the macro call site.
///
/// The top-level form must be a component (`(component ...)`); a `(module
/// ...)` form is rejected with a clear error pointing at [`wasm!`].
///
/// # Example
///
/// ```ignore
/// const HELLO: &[u8] = wcmp_macros::component!(r#"
///     (component
///       (core module $m
///         (func (export "f") (result i32) i32.const 42))
///       (core instance $i (instantiate $m))
///       (func (export "f") (canon lift (core func $i "f"))))
/// "#);
/// ```
#[proc_macro]
pub fn component(input: TokenStream) -> TokenStream {
    let literal = parse_macro_input!(input as LitStr);
    assemble(literal, BinaryKind::Component).into()
}

#[derive(Clone, Copy)]
enum BinaryKind {
    Module,
    Component,
}

impl BinaryKind {
    fn description(self) -> &'static str {
        match self {
            BinaryKind::Module => "core module",
            BinaryKind::Component => "component",
        }
    }

    fn sibling_macro(self) -> &'static str {
        match self {
            BinaryKind::Module => "component!",
            BinaryKind::Component => "wasm!",
        }
    }

    /// The 4-byte version word that follows `\0asm` in the binary header.
    /// `\x01\0\0\0` for core modules; `\x0d\0\x01\0` for components.
    fn classify(bytes: &[u8]) -> Option<BinaryKind> {
        if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
            return None;
        }
        match &bytes[4..8] {
            [0x01, 0x00, 0x00, 0x00] => Some(BinaryKind::Module),
            [0x0d, 0x00, 0x01, 0x00] => Some(BinaryKind::Component),
            _ => None,
        }
    }
}

fn assemble(literal: LitStr, expected: BinaryKind) -> TokenStream2 {
    let span = literal.span();
    let source = literal.value();

    let bytes = match wat::parse_str(&source) {
        Ok(bytes) => bytes,
        Err(err) => return error(span, format!("WAT parse error: {err}")),
    };

    match BinaryKind::classify(&bytes) {
        Some(actual) if matches_kind(actual, expected) => {}
        Some(actual) => {
            return error(
                span,
                format!(
                    "expected a {}, but the WAT assembled to a {}; use `{}` instead",
                    expected.description(),
                    actual.description(),
                    expected.sibling_macro(),
                ),
            );
        }
        None => {
            return error(
                span,
                "WAT assembled to bytes that are neither a core module nor a component".to_string(),
            );
        }
    }

    let byte_literals = bytes.iter().map(|b| quote!(#b));
    quote! {
        &[#(#byte_literals),*] as &'static [u8]
    }
}

fn matches_kind(actual: BinaryKind, expected: BinaryKind) -> bool {
    matches!(
        (actual, expected),
        (BinaryKind::Module, BinaryKind::Module) | (BinaryKind::Component, BinaryKind::Component)
    )
}

fn error(span: Span, message: String) -> TokenStream2 {
    syn::Error::new(span, message).to_compile_error()
}
