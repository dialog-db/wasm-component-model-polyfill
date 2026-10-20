// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Rust names of WIT names, as Wasmtime's macro spells them
//! (`crates/wit-bindgen/src/rust.rs` and `lib.rs` at `v50.0.0`).

use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Ident, Span};
use wit_parser::{Function, FunctionKind, Resolve, Type, TypeDefKind, TypeId};

/// The Rust words that cannot name an item, each of which a WIT name
/// becomes with a trailing `_`.
const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

/// `name` as a Rust `snake_case` name, with a keyword escaped.
pub fn to_rust_ident(name: &str) -> String {
    if KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_snake_case()
    }
}

/// `name` as a Rust `UpperCamelCase` name. `host` becomes `Host_`,
/// because `Host` is the name of the traits the macro writes.
pub fn to_rust_upper_camel_case(name: &str) -> String {
    match name {
        "host" => "Host_".to_owned(),
        name => name.to_upper_camel_case(),
    }
}

/// An identifier with the call site's span.
pub fn ident(name: &str) -> Ident {
    Ident::new(name, Span::call_site())
}

/// The name of the trait method of `func`: `new` for a constructor,
/// and the function's own name, without its resource, otherwise.
pub fn rust_function_name(func: &Function) -> String {
    match func.kind {
        FunctionKind::Constructor(_) => "new".to_owned(),
        _ => to_rust_ident(func.item_name()),
    }
}

/// The name the world struct keeps the export `func` under, which is
/// also the suffix of its `call_*` method: the resource's name joins
/// a method's, a static function's, or a constructor's.
pub fn func_field_name(resolve: &Resolve, func: &Function) -> String {
    let mut name = String::new();
    let mut resource = |kind: &str, id: TypeId| {
        name.push_str(kind);
        name.push('-');
        name.push_str(resolve.types[id].name.as_deref().unwrap_or_default());
        name.push('-');
    };
    match func.kind {
        FunctionKind::Method(id) | FunctionKind::AsyncMethod(id) => resource("method", id),
        FunctionKind::Static(id) | FunctionKind::AsyncStatic(id) => resource("static", id),
        FunctionKind::Constructor(id) => resource("constructor", id),
        FunctionKind::Freestanding | FunctionKind::AsyncFreestanding => {}
        FunctionKind::Getter
        | FunctionKind::Setter
        | FunctionKind::MethodGetter(_)
        | FunctionKind::MethodSetter(_)
        | FunctionKind::StaticGetter(_)
        | FunctionKind::StaticSetter(_) => {
            unreachable!("the generator refuses a getter or a setter before it names one")
        }
    }
    name.push_str(func.item_name());
    name.to_snake_case()
}

/// The type a chain of aliases starting at `id` ends at.
pub fn resolve_type_definition_id(resolve: &Resolve, mut id: TypeId) -> TypeId {
    while let TypeDefKind::Type(Type::Id(next)) = resolve.types[id].kind {
        id = next;
    }
    id
}
