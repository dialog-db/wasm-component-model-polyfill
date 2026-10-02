// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Procedural macros for the Wasm Component Model Polyfill workspace.
//!
//! Four macros live here:
//!
//! - [`macro@test`] — cross-target attribute that expands to `#[tokio::test]`
//!   on native, or to the built-in `#[test]` for a synchronous body, and to
//!   `#[wasm_bindgen_test]` on `wasm32-unknown-unknown`.
//! - [`macro@bench`] — attribute that turns one benchmark body into the
//!   descriptors a benchmark suite drives unchanged on either target.
//! - [`wasm!`] — assemble inline WebAssembly Text Format into a core-module
//!   byte slice at compile time.
//! - [`component!`] — assemble inline WebAssembly Text Format into a
//!   component-format byte slice at compile time.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, ItemFn, Lit, LitStr, MetaNameValue, Token, Visibility, parse_macro_input};

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

/// Benchmark attribute.
///
/// The attribute marks an `async fn` as one benchmark definition that the
/// suite's runner drives unchanged on both targets. The body takes the
/// benchmark's run and returns the suite's `Result<()>`:
///
/// ```ignore
/// #[wcmp_macros::bench(
///     guest = "the `guest` corpus fixture",
///     payload = "one u32 in, one u32 out",
/// )]
/// async fn u32_call(run: &mut Run) -> Result<()> {
///     let (mut store, function) = setup().await?;
///     while run.iterate() {
///         function.call(&mut store, &[Val::U32(21)]).await?;
///     }
///     Ok(())
/// }
/// ```
///
/// `guest` names the guest the benchmark drives and `payload` the value it
/// moves; both are required, and both reach the report so that a number has
/// a meaning. The optional `cases` argument repeats one definition over
/// several payload sizes, or over several named guests:
///
/// ```ignore
/// #[wcmp_macros::bench(guest = "...", payload = "...", cases = [64, 4096])]
/// async fn string_roundtrip(run: &mut Run) -> Result<()> {
///     let size = run.case().number();
///     // ...
/// }
/// ```
///
/// The attribute expands to a function of the same name that takes no
/// arguments and returns one benchmark descriptor per case, so a suite lists
/// its benchmarks by calling them. A numeric case is named
/// `<benchmark>/<number>` and a string case `<benchmark>/<name>`; underscores
/// in the function's name become hyphens.
#[proc_macro_attribute]
pub fn bench(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    match expand_bench(attr.into(), function) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The `bench` expansion: the benchmark body nested inside the descriptor
/// function that names it.
fn expand_bench(attr: TokenStream2, function: ItemFn) -> syn::Result<TokenStream2> {
    let arguments = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(attr)?;
    let mut guest = None;
    let mut payload = None;
    let mut cases = Vec::new();
    for argument in &arguments {
        let key = argument
            .path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_default();
        match key.as_str() {
            "guest" => guest = Some(string_argument(&argument.value)?),
            "payload" => payload = Some(string_argument(&argument.value)?),
            "cases" => cases = case_arguments(&argument.value)?,
            _ => {
                return Err(syn::Error::new_spanned(
                    &argument.path,
                    "`#[wcmp_macros::bench]` takes `guest`, `payload`, and `cases`",
                ));
            }
        }
    }
    let guest = guest.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "`#[wcmp_macros::bench]` needs `guest = \"...\"`: the guest the benchmark drives",
        )
    })?;
    let payload = payload.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "`#[wcmp_macros::bench]` needs `payload = \"...\"`: the value the benchmark moves",
        )
    })?;
    if cases.is_empty() {
        cases.push(quote!(::wcmp_bench::Case::None));
    }

    let name = function.sig.ident.to_string().replace('_', "-");
    let ident = function.sig.ident.clone();
    let visibility = function.vis.clone();
    let mut body = function;
    body.vis = Visibility::Inherited;

    // The body keeps the author's name and the descriptor function takes it
    // too: an item declared in a block shadows the enclosing one, so the
    // thunk below reaches the benchmark and not itself.
    Ok(quote! {
        #visibility fn #ident() -> ::std::vec::Vec<::wcmp_bench::Benchmark> {
            #body

            fn thunk(
                run: &mut ::wcmp_bench::Run,
            ) -> ::std::pin::Pin<
                ::std::boxed::Box<
                    dyn ::std::future::Future<Output = ::wcmp_bench::Result<()>> + '_,
                >,
            > {
                ::std::boxed::Box::pin(#ident(run))
            }

            ::wcmp_bench::Benchmark::cases(#name, #guest, #payload, &[#(#cases),*], thunk)
        }
    })
}

/// A `key = "..."` argument's string.
fn string_argument(value: &Expr) -> syn::Result<LitStr> {
    match value {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(text) => Ok(text.clone()),
            other => Err(syn::Error::new_spanned(other, "expected a string literal")),
        },
        other => Err(syn::Error::new_spanned(other, "expected a string literal")),
    }
}

/// A `cases = [...]` argument's cases: integer literals for payload sizes,
/// string literals for named guests.
fn case_arguments(value: &Expr) -> syn::Result<Vec<TokenStream2>> {
    let Expr::Array(array) = value else {
        return Err(syn::Error::new_spanned(
            value,
            "expected an array of integer or string literals",
        ));
    };
    array
        .elems
        .iter()
        .map(|element| match element {
            Expr::Lit(literal) => match &literal.lit {
                Lit::Int(number) => {
                    let number = number.base10_parse::<u64>()?;
                    Ok(quote!(::wcmp_bench::Case::Number(#number)))
                }
                Lit::Str(text) => Ok(quote!(::wcmp_bench::Case::Name(#text))),
                other => Err(syn::Error::new_spanned(
                    other,
                    "expected an integer or string literal",
                )),
            },
            other => Err(syn::Error::new_spanned(
                other,
                "expected an integer or string literal",
            )),
        })
        .collect()
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
