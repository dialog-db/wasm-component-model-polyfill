// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `bindgen!` macro of the Wasm Component Model Polyfill, which the
//! polyfill re-exports as `wcmp::bindgen!`.
//!
//! The macro reads WIT and writes the Rust code that connects a host to
//! a component, with the names and the workflow of Wasmtime's
//! `wasmtime::component::bindgen!`, against the polyfill's own API. It
//! is the polyfill's own generator over `wit-parser`, and it runs at
//! compile time on the build machine, so no browser binary contains
//! the parser. Wasmtime's generator, `crates/wit-bindgen` at
//! `v49.0.0-rc.1`, is the reference for its structure and its names.
//!
//! See the polyfill's documentation of `wcmp::bindgen!` for the
//! invocation forms, the options, and the code the macro writes.

mod config;
mod direction;
mod exports;
mod function_config;
mod function_flags;
mod generator;
mod imports;
mod interface_name;
mod ledger;
mod lookup;
mod module_tree;
mod names;
mod options;
mod ownership;
#[cfg(test)]
mod tests;
mod trait_extras;
mod trappable_error;
mod trappable_errors;
mod type_info;
mod type_mode;
mod with;
mod with_entry;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use proc_macro2::TokenStream;
use quote::quote;

use crate::config::Config;
use crate::generator::Generator;
use crate::options::Options;

/// Generate host bindings for a WIT world, as Wasmtime's
/// `wasmtime::component::bindgen!` does, against the polyfill's API.
///
/// # Invocation
///
/// The macro takes Wasmtime's five forms:
///
/// - `bindgen!()` reads `wit/` beside the invoking crate's
///   `Cargo.toml`, and generates its one world.
/// - `bindgen!("world")` reads `wit/` and generates the world named.
/// - `bindgen!(in "path")` reads the path, relative to that
///   `Cargo.toml`.
/// - `bindgen!("world" in "path")` does both.
/// - `bindgen!({ … })` takes the options below.
///
/// Every item behind a WIT feature gate generates, as though every
/// feature is on, as Wasmtime's macro reads them. The macro writes the
/// same code for every target: its paths name `wcmp`, and nothing it
/// writes depends on the target.
///
/// # The code it writes
///
/// Each interface is a module at Wasmtime's path: `example:app/log` is
/// `example::app::log`, an exported interface sits under `exports`,
/// and a package with two versions in the WIT carries its version in
/// its module's name, as `dep0_1_0`. Each WIT type is a Rust type with
/// the polyfill's derives, `ComponentType`, `Lift`, and `Lower`, or
/// `flags!`, with its size and alignment asserted against the WIT's.
/// An imported resource is the empty enum `pub enum Name {}`, and its
/// handle is `Resource<Name>`.
///
/// For each imported interface the module has:
///
/// - A trait `Host`, with one method for each function without the
///   `store` flag, which the host implements on what its projection
///   gives.
/// - A trait `HostWithStore<T>: HasData`, with one method for each
///   function with the `store` flag. Each takes an
///   `wcmp::Access` in place of `self`, and the
///   host implements it on its `HasData` type. With no such function,
///   every `HasData` type implements it.
/// - `add_to_linker_instance` and `add_to_linker`, which register each
///   function, given a projection `fn(&mut T) -> D::Data<'_>`.
///
/// The functions a world imports outside an interface are in the
/// traits `{World}Imports` and `{World}ImportsWithStore<T>`, which
/// `add_to_linker_imports` registers.
///
/// An imported WIT `async func` is a method of `HostWithStore<T>` that
/// takes `&Accessor<T, Self>` and returns a future, registered through
/// `func_wrap_concurrent`. Natively the future and the traits are
/// `Send`; in the browser they need not be, because the bound is
/// `wcmp::MaybeSend`.
///
/// An imported resource `r` has the traits `HostR`, with its
/// constructor, its methods, its static functions, and `drop`, and
/// `HostRWithStore<T>`, which the interface's traits extend. Its
/// registration names `ResourceType::host::<R>()`. A resource another
/// interface `use`s is the same resource, registered once, where it is
/// defined.
///
/// Each exported interface has a struct `Guest`, with a `call_*` method
/// for each function and a getter `r()` for each exported resource `r`,
/// whose calls take its `ResourceAny`. The world has a struct named
/// after it, with `instantiate_async`, `new`, `add_to_linker`, one getter
/// for each exported interface, and a `call_*` method for each function
/// the world exports. `new` acquires every export as a `TypedFunc`, so
/// the type checks run there and never in a call; a missing export
/// fails it with `Error::MissingExport`. Every call into a component is
/// an `async fn`. A `call_*` method takes `&mut Store<T>` and borrowed
/// parameters (`&str`, `&[T]`, `&T`), which it copies into the call. An
/// export with the `async` and `store` flags, and every WIT `async func`
/// export, takes an `impl AsAccessor` and owned parameters, and calls
/// `call_concurrent`. The world has no `instantiate`, `{World}Pre`, or
/// `{World}Indices`.
///
/// # Options
///
/// | Option | |
/// | --- | --- |
/// | `world`, `path`, `inline`, `interfaces` | the WIT, as in Wasmtime; `interfaces` writes no world struct |
/// | `imports`, `exports` | the flags of each function, as in Wasmtime: the first rule that matches a function applies, `default` applies when none does, and a rule that matches no function fails the compile |
/// | `with` | an interface, a package, a namespace, a version of a package, or one resource, mapped to Rust code; the most specific key applies, and a key that matches nothing fails the compile. A mapped interface's module must be one `wcmp::bindgen!` wrote: a module from Wasmtime's macro names Wasmtime's types, and the code does not compile |
/// | `trappable_error_type` | the Rust error a trappable function that returns that WIT error returns, which `convert_*` turns back |
/// | `additional_derives` | derives each generated record, variant, and enum carries |
/// | `stringify` | the generated code, as a string |
/// | `debug`, `include_generated_code_from_file` | the generated code, from a file the macro writes |
/// | `skip_mut_forwarding_impls` | no `impl Host for &mut T` |
/// | `require_store_data_send` | `T: Send` natively, and nothing in the browser |
/// | `anyhow` | trappable methods return `anyhow::Result`; an error that holds a polyfill `Error` comes back as it, any other as `Error::Host` |
/// | `ownership: Owning` | the default |
/// | `wcmp_crate` | the path of the polyfill, for a crate that re-exports it |
///
/// The flags of a function are `trappable` (the method returns a
/// `Result`, and an `Err` traps the guest), `store` (the method takes an
/// `Access` for the store), `tracing`, `verbose_tracing`, and `exact`.
/// `async` on an export changes nothing, and with `store` it gives the
/// accessor form.
///
/// The macro refuses, with a compile error that names the option and
/// the reason: `async` on a plain import, because the polyfill has no
/// host function that holds the store across an await (declare the
/// function `async func` instead); `async` on a destructor;
/// `ownership: Borrowing { … }`; `named_imports`;
/// `include_component_type`; and `wasmtime_crate`, which names
/// `wcmp_crate`.
#[proc_macro]
pub fn bindgen(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let root = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    expand(input.into(), &root)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// The code an invocation whose tokens are `input` writes, reading its
/// WIT relative to `root`.
fn expand(input: TokenStream, root: &Path) -> syn::Result<TokenStream> {
    let options: Options = syn::parse2(input)?;
    let config = Config::new(options, root)?;
    let code = Generator::new(&config).generate()?;

    if config.options.stringify {
        let text = code.to_string();
        return Ok(quote!(#text));
    }

    let code = if config.options.include_generated_code_from_file || config.options.debug {
        include_from_file(&config, &code)?
    } else {
        code
    };

    // Each file the WIT was read from is named, so that a change to one
    // rebuilds the invoking crate.
    let files = config.files.iter().map(|file| {
        let file = file.display().to_string();
        quote!(
            const _: &[u8] = include_bytes!(#file);
        )
    });
    Ok(quote! {
        #code
        #(#files)*
    })
}

/// Write `code` to a file and give the `include!` of it, which is what
/// `include_generated_code_from_file` and `debug` ask for: a compile
/// error in the generated code then names a line of a file a person
/// can read.
fn include_from_file(config: &Config, code: &TokenStream) -> syn::Result<TokenStream> {
    static INVOCATION: AtomicUsize = AtomicUsize::new(0);
    let world = &config.resolve.worlds[config.world].name;
    let number = INVOCATION.fetch_add(1, Ordering::Relaxed);
    let path = Path::new(env!("WCMP_BINDGEN_OUTPUT_DIR")).join(format!("{world}{number}.rs"));
    std::fs::write(&path, code.to_string()).map_err(|error| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "cannot write the generated code to {}: {error}",
                path.display()
            ),
        )
    })?;
    // A formatted file reads better, and an unformatted one still
    // compiles, so a missing `rustfmt` is not an error.
    drop(
        std::process::Command::new("rustfmt")
            .arg("--edition=2024")
            .arg(&path)
            .output(),
    );
    let path = path.display().to_string();
    Ok(quote!(include!(#path);))
}
