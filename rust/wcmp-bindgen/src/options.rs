// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The options of one invocation of the macro.

use proc_macro2::Span;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{LitBool, LitStr, Token, braced, bracketed, token};

use crate::function_config::FunctionConfig;
use crate::function_flags::FunctionFlags;
use crate::ownership::Ownership;
use crate::trappable_error::TrappableError;
use crate::with_entry::WithEntry;

mod kw {
    syn::custom_keyword!(inline);
    syn::custom_keyword!(path);
    syn::custom_keyword!(tracing);
    syn::custom_keyword!(verbose_tracing);
    syn::custom_keyword!(trappable_error_type);
    syn::custom_keyword!(world);
    syn::custom_keyword!(ownership);
    syn::custom_keyword!(interfaces);
    syn::custom_keyword!(with);
    syn::custom_keyword!(named_imports);
    syn::custom_keyword!(additional_derives);
    syn::custom_keyword!(stringify);
    syn::custom_keyword!(skip_mut_forwarding_impls);
    syn::custom_keyword!(require_store_data_send);
    syn::custom_keyword!(wasmtime_crate);
    syn::custom_keyword!(wcmp_crate);
    syn::custom_keyword!(anyhow);
    syn::custom_keyword!(include_generated_code_from_file);
    syn::custom_keyword!(include_component_type);
    syn::custom_keyword!(debug);
    syn::custom_keyword!(imports);
    syn::custom_keyword!(exports);
    syn::custom_keyword!(store);
    syn::custom_keyword!(trappable);
    syn::custom_keyword!(exact);
}

/// The options of one invocation, as Wasmtime's macro parses them
/// (`crates/component-macro/src/bindgen.rs` at `v49.0.0-rc.1`), with
/// the polyfill's `wcmp_crate`.
#[derive(Default)]
pub struct Options {
    /// `world`, or the world of `bindgen!("world")`.
    pub world: Option<LitStr>,
    /// `path`, or the path of `bindgen!(in "path")`.
    pub paths: Vec<LitStr>,
    /// `inline`, or the world `interfaces` makes.
    pub inline: Option<String>,
    /// Whether `interfaces` named the interfaces to generate.
    pub only_interfaces: bool,
    /// `debug`.
    pub debug: bool,
    /// `stringify`.
    pub stringify: bool,
    /// `include_generated_code_from_file`.
    pub include_generated_code_from_file: bool,
    /// `additional_derives`.
    pub additional_derives: Vec<syn::Path>,
    /// `wcmp_crate`.
    pub wcmp_crate: Option<syn::Path>,
    /// `imports`.
    pub imports: FunctionConfig,
    /// `exports`.
    pub exports: FunctionConfig,
    /// `with`.
    pub with: Vec<WithEntry>,
    /// `trappable_error_type`.
    pub trappable_error_type: Vec<TrappableError>,
    /// `ownership`.
    pub ownership: Ownership,
    /// `named_imports`, where it was written. Parsed, and refused.
    pub named_imports: Option<Span>,
    /// `include_component_type: true`, where it was written. Parsed,
    /// and refused.
    pub include_component_type: Option<Span>,
    /// `anyhow`.
    pub anyhow: bool,
    /// `skip_mut_forwarding_impls`.
    pub skip_mut_forwarding_impls: bool,
    /// `require_store_data_send`.
    pub require_store_data_send: bool,
}

impl Parse for Options {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut options = Options::default();
        if !input.peek(token::Brace) {
            // The short forms: `bindgen!()`, `bindgen!("world")`,
            // `bindgen!(in "path")`, and `bindgen!("world" in "path")`.
            options.world = input.parse::<Option<LitStr>>()?;
            if input.parse::<Option<Token![in]>>()?.is_some() {
                options.paths.push(input.parse::<LitStr>()?);
            }
            return Ok(options);
        }

        let content;
        braced!(content in input);
        let mut imports = None;
        let mut exports = None;
        for option in Punctuated::<Opt, Token![,]>::parse_terminated(&content)? {
            match option {
                Opt::World(world) => {
                    if options.world.is_some() {
                        return Err(syn::Error::new(world.span(), "cannot specify second world"));
                    }
                    options.world = Some(world);
                }
                Opt::Path(paths) => options.paths.extend(paths),
                Opt::Inline(source) => {
                    if options.inline.is_some() {
                        return Err(syn::Error::new(
                            source.span(),
                            "cannot specify second source",
                        ));
                    }
                    options.inline = Some(source.value());
                }
                Opt::Interfaces(interfaces) => {
                    if options.inline.is_some() {
                        return Err(syn::Error::new(
                            interfaces.span(),
                            "cannot specify a second source",
                        ));
                    }
                    if options.world.is_some() {
                        return Err(syn::Error::new(
                            interfaces.span(),
                            "cannot specify a world with `interfaces`",
                        ));
                    }
                    options.inline = Some(format!(
                        "package wcmp:bindgen-synthesized;\n\nworld interfaces {{\n{}\n}}\n",
                        interfaces.value()
                    ));
                    options.world = Some(LitStr::new(
                        "wcmp:bindgen-synthesized/interfaces",
                        interfaces.span(),
                    ));
                    options.only_interfaces = true;
                }
                Opt::Debug(value) => options.debug = value,
                Opt::Stringify(value) => options.stringify = value,
                Opt::IncludeGeneratedCodeFromFile(value) => {
                    options.include_generated_code_from_file = value;
                }
                Opt::AdditionalDerives(paths) => options.additional_derives.extend(paths),
                Opt::WcmpCrate(path) => options.wcmp_crate = Some(path),
                Opt::Imports(config, span) => {
                    if imports.replace(config).is_some() {
                        return Err(syn::Error::new(
                            span,
                            "cannot specify imports configuration",
                        ));
                    }
                }
                Opt::Exports(config, span) => {
                    if exports.replace(config).is_some() {
                        return Err(syn::Error::new(
                            span,
                            "cannot specify exports configuration",
                        ));
                    }
                }
                Opt::With(entries) => options.with.extend(entries),
                Opt::TrappableErrorType(entries) => options.trappable_error_type.extend(entries),
                Opt::Ownership(ownership) => options.ownership = ownership,
                Opt::NamedImports(span) => options.named_imports = Some(span),
                Opt::IncludeComponentType(value, span) => {
                    options.include_component_type = value.then_some(span);
                }
                Opt::Anyhow(value) => options.anyhow = value,
                Opt::SkipMutForwardingImpls(value) => options.skip_mut_forwarding_impls = value,
                Opt::RequireStoreDataSend(value) => options.require_store_data_send = value,
            }
        }
        options.imports = imports.unwrap_or_default();
        options.exports = exports.unwrap_or_default();
        Ok(options)
    }
}

/// One option of the braced form.
enum Opt {
    World(LitStr),
    Path(Vec<LitStr>),
    Inline(LitStr),
    Interfaces(LitStr),
    Debug(bool),
    Stringify(bool),
    IncludeGeneratedCodeFromFile(bool),
    AdditionalDerives(Vec<syn::Path>),
    WcmpCrate(syn::Path),
    Imports(FunctionConfig, Span),
    Exports(FunctionConfig, Span),
    With(Vec<WithEntry>),
    TrappableErrorType(Vec<TrappableError>),
    Ownership(Ownership),
    NamedImports(Span),
    IncludeComponentType(bool, Span),
    Anyhow(bool),
    SkipMutForwardingImpls(bool),
    RequireStoreDataSend(bool),
}

/// Parse `: true` or `: false`.
fn boolean(input: ParseStream<'_>) -> syn::Result<bool> {
    input.parse::<Token![:]>()?;
    Ok(input.parse::<LitBool>()?.value)
}

impl Parse for Opt {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let l = input.lookahead1();
        if l.peek(kw::debug) {
            input.parse::<kw::debug>()?;
            Ok(Opt::Debug(boolean(input)?))
        } else if l.peek(kw::path) {
            input.parse::<kw::path>()?;
            input.parse::<Token![:]>()?;
            let l = input.lookahead1();
            if l.peek(LitStr) {
                Ok(Opt::Path(vec![input.parse()?]))
            } else if l.peek(token::Bracket) {
                let content;
                bracketed!(content in input);
                let paths = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                Ok(Opt::Path(paths.into_iter().collect()))
            } else {
                Err(l.error())
            }
        } else if l.peek(kw::inline) {
            input.parse::<kw::inline>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Inline(input.parse()?))
        } else if l.peek(kw::world) {
            input.parse::<kw::world>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::World(input.parse()?))
        } else if l.peek(kw::ownership) {
            input.parse::<kw::ownership>()?;
            input.parse::<Token![:]>()?;
            parse_ownership(input).map(Opt::Ownership)
        } else if l.peek(kw::trappable_error_type) {
            input.parse::<kw::trappable_error_type>()?;
            input.parse::<Token![:]>()?;
            let content;
            braced!(content in input);
            let entries = content.parse_terminated(parse_trappable_error, Token![,])?;
            Ok(Opt::TrappableErrorType(entries.into_iter().collect()))
        } else if l.peek(kw::interfaces) {
            input.parse::<kw::interfaces>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Interfaces(input.parse()?))
        } else if l.peek(kw::with) {
            input.parse::<kw::with>()?;
            input.parse::<Token![:]>()?;
            let content;
            braced!(content in input);
            let entries = content.parse_terminated(parse_with_entry, Token![,])?;
            Ok(Opt::With(entries.into_iter().collect()))
        } else if l.peek(kw::named_imports) {
            let span = input.parse::<kw::named_imports>()?.span;
            input.parse::<Token![:]>()?;
            let content;
            braced!(content in input);
            content.parse_terminated(parse_with_entry, Token![,])?;
            Ok(Opt::NamedImports(span))
        } else if l.peek(kw::additional_derives) {
            input.parse::<kw::additional_derives>()?;
            input.parse::<Token![:]>()?;
            let content;
            bracketed!(content in input);
            let paths = Punctuated::<syn::Path, Token![,]>::parse_terminated(&content)?;
            Ok(Opt::AdditionalDerives(paths.into_iter().collect()))
        } else if l.peek(kw::stringify) {
            input.parse::<kw::stringify>()?;
            Ok(Opt::Stringify(boolean(input)?))
        } else if l.peek(kw::skip_mut_forwarding_impls) {
            input.parse::<kw::skip_mut_forwarding_impls>()?;
            Ok(Opt::SkipMutForwardingImpls(boolean(input)?))
        } else if l.peek(kw::require_store_data_send) {
            input.parse::<kw::require_store_data_send>()?;
            Ok(Opt::RequireStoreDataSend(boolean(input)?))
        } else if l.peek(kw::wasmtime_crate) {
            let span = input.parse::<kw::wasmtime_crate>()?.span;
            Err(syn::Error::new(
                span,
                format!(
                    "`wasmtime_crate` is not supported: {}",
                    crate::ledger::WASMTIME_CRATE
                ),
            ))
        } else if l.peek(kw::wcmp_crate) {
            input.parse::<kw::wcmp_crate>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::WcmpCrate(input.parse()?))
        } else if l.peek(kw::anyhow) {
            input.parse::<kw::anyhow>()?;
            Ok(Opt::Anyhow(boolean(input)?))
        } else if l.peek(kw::include_generated_code_from_file) {
            input.parse::<kw::include_generated_code_from_file>()?;
            Ok(Opt::IncludeGeneratedCodeFromFile(boolean(input)?))
        } else if l.peek(kw::include_component_type) {
            let span = input.parse::<kw::include_component_type>()?.span;
            Ok(Opt::IncludeComponentType(boolean(input)?, span))
        } else if l.peek(kw::imports) {
            let span = input.parse::<kw::imports>()?.span;
            input.parse::<Token![:]>()?;
            Ok(Opt::Imports(parse_function_config(input)?, span))
        } else if l.peek(kw::exports) {
            let span = input.parse::<kw::exports>()?.span;
            input.parse::<Token![:]>()?;
            Ok(Opt::Exports(parse_function_config(input)?, span))
        } else {
            Err(l.error())
        }
    }
}

/// Parse `Owning` or `Borrowing { duplicate_if_necessary: bool }`.
fn parse_ownership(input: ParseStream<'_>) -> syn::Result<Ownership> {
    let name = input.parse::<syn::Ident>()?;
    match name.to_string().as_str() {
        "Owning" => Ok(Ownership::Owning),
        "Borrowing" => {
            let content;
            braced!(content in input);
            let field = content.parse::<syn::Ident>()?;
            if field != "duplicate_if_necessary" {
                return Err(syn::Error::new(
                    field.span(),
                    format!(
                        "unrecognized `Ownership::Borrowing` field: `{field}`; \
                         expected `duplicate_if_necessary`"
                    ),
                ));
            }
            content.parse::<Token![:]>()?;
            content.parse::<LitBool>()?;
            Ok(Ownership::Borrowing { span: name.span() })
        }
        other => Err(syn::Error::new(
            name.span(),
            format!("unrecognized ownership: `{other}`; expected `Owning` or `Borrowing`"),
        )),
    }
}

/// Parse one `"wit-path" => RustType` entry of `trappable_error_type`.
fn parse_trappable_error(input: ParseStream<'_>) -> syn::Result<TrappableError> {
    let wit_path = input.parse::<LitStr>()?;
    input.parse::<Token![=>]>()?;
    let rust_type = input.parse::<syn::Path>()?;
    Ok(TrappableError {
        wit_path: wit_path.value(),
        rust_type,
        span: wit_path.span(),
    })
}

/// Parse one `"wit-name": rust::path` entry of `with`.
fn parse_with_entry(input: ParseStream<'_>) -> syn::Result<WithEntry> {
    let key = input.parse::<LitStr>()?;
    input.parse::<Token![:]>()?;
    let path = input.parse::<syn::Path>()?;
    if path
        .segments
        .iter()
        .any(|segment| !segment.arguments.is_none())
    {
        return Err(syn::Error::new_spanned(
            &path,
            "Module path must not contain angles or parens",
        ));
    }
    Ok(WithEntry {
        key: key.value(),
        path,
        span: key.span(),
    })
}

/// Parse the braced rules of `imports` or `exports`.
fn parse_function_config(input: ParseStream<'_>) -> syn::Result<FunctionConfig> {
    let content;
    braced!(content in input);
    let mut config = FunctionConfig::default();
    for rule in Punctuated::<Rule, Token![,]>::parse_terminated(&content)? {
        match rule.filter {
            Some(filter) => config.push(filter.value(), rule.flags, filter.span()),
            None => config.set_default(rule.flags),
        }
    }
    Ok(config)
}

/// One rule of `imports` or `exports`: a name or `default`, and flags.
struct Rule {
    filter: Option<LitStr>,
    flags: FunctionFlags,
}

impl Parse for Rule {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let l = input.lookahead1();
        let filter = if l.peek(LitStr) {
            Some(input.parse::<LitStr>()?)
        } else if l.peek(Token![default]) {
            input.parse::<Token![default]>()?;
            None
        } else {
            return Err(l.error());
        };
        input.parse::<Token![:]>()?;
        let mut flags = FunctionFlags::empty();
        while !input.is_empty() {
            let l = input.lookahead1();
            if l.peek(Token![async]) {
                input.parse::<Token![async]>()?;
                flags |= FunctionFlags::ASYNC;
            } else if l.peek(kw::tracing) {
                input.parse::<kw::tracing>()?;
                flags |= FunctionFlags::TRACING;
            } else if l.peek(kw::verbose_tracing) {
                input.parse::<kw::verbose_tracing>()?;
                flags |= FunctionFlags::VERBOSE_TRACING;
            } else if l.peek(kw::store) {
                input.parse::<kw::store>()?;
                flags |= FunctionFlags::STORE;
            } else if l.peek(kw::trappable) {
                input.parse::<kw::trappable>()?;
                flags |= FunctionFlags::TRAPPABLE;
            } else if l.peek(kw::exact) {
                input.parse::<kw::exact>()?;
                flags |= FunctionFlags::EXACT;
            } else {
                return Err(l.error());
            }
            if input.peek(Token![|]) {
                input.parse::<Token![|]>()?;
            } else {
                break;
            }
        }
        Ok(Rule { filter, flags })
    }
}
