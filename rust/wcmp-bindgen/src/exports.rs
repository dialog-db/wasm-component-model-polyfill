// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The calls of the functions a world exports.

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use wit_parser::{Function, Type, TypeDefKind, WorldKey};

use crate::function_flags::FunctionFlags;
use crate::generator::Generator;
use crate::names::{func_field_name, ident};
use crate::type_mode::TypeMode;

/// The code of one exported function: the field that keeps its
/// `TypedFunc`, the acquisition of the field, and its methods.
pub struct ExportedFunction {
    /// The field, as `name: TypedFunc<…>`.
    pub field: TokenStream,
    /// The acquisition of the field from `lookup`, as `name: …?`.
    pub acquire: TokenStream,
    /// `func_*` and `call_*`.
    pub methods: TokenStream,
}

impl Generator<'_> {
    /// The flags the `exports` option gives `func`.
    pub fn export_flags(&self, key: Option<&WorldKey>, func: &Function) -> FunctionFlags {
        self.config.options.exports.flags(self.resolve, key, func)
    }

    /// Whether a value of `ty` can be cloned: it holds no handle, no
    /// stream, and no future. An export takes a borrow of such a value
    /// and clones it into the call; it takes any other by value.
    fn clonable(&self, ty: &Type) -> bool {
        match ty {
            Type::Id(id) => self.infos.get(*id).is_clone(),
            _ => true,
        }
    }

    /// The type an export's call takes for a parameter of `ty`:
    /// Wasmtime's shallow borrow (`&str`, `&[T]`, `&T` for a named type
    /// that holds a list) where the value can be cloned, and the value
    /// itself otherwise.
    pub fn param_ty(&self, scope: &[String], ty: &Type, mode: TypeMode) -> TokenStream {
        if mode == TypeMode::Owned {
            return self.ty(scope, ty, TypeMode::Owned);
        }
        match ty {
            Type::String => quote!(&str),
            Type::Id(id) => {
                let def = &self.resolve.types[*id];
                if def.name.is_some() {
                    let owned = self.ty(scope, ty, TypeMode::Owned);
                    return if self.infos.get(*id).has_list && self.clonable(ty) {
                        quote!(&#owned)
                    } else {
                        owned
                    };
                }
                match &def.kind {
                    TypeDefKind::List(element) if self.clonable(element) => {
                        let element = self.ty(scope, element, TypeMode::Owned);
                        quote!(&[#element])
                    }
                    TypeDefKind::Option(payload) => {
                        let payload = self.param_ty(scope, payload, mode);
                        quote!(::core::option::Option<#payload>)
                    }
                    TypeDefKind::Result(result) => {
                        let ok = match &result.ok {
                            Some(ok) => self.param_ty(scope, ok, mode),
                            None => quote!(()),
                        };
                        let err = match &result.err {
                            Some(err) => self.param_ty(scope, err, mode),
                            None => quote!(()),
                        };
                        quote!(::core::result::Result<#ok, #err>)
                    }
                    TypeDefKind::Tuple(tuple) => {
                        let types = tuple.types.iter().map(|ty| self.param_ty(scope, ty, mode));
                        quote!((#(#types,)*))
                    }
                    TypeDefKind::Type(alias) => self.param_ty(scope, alias, mode),
                    _ => self.ty(scope, ty, TypeMode::Owned),
                }
            }
            _ => self.ty(scope, ty, TypeMode::Owned),
        }
    }

    /// The owned value of `ty` from `value`, a parameter of the type
    /// [`Self::param_ty`] gives in `mode`.
    ///
    /// A typed call keeps its arguments until the task it starts runs,
    /// so they are owned: a borrowed argument is copied once on the
    /// host, as the guest's memory would hold it in any case.
    pub fn to_owned(
        &self,
        scope: &[String],
        ty: &Type,
        mode: TypeMode,
        value: TokenStream,
    ) -> TokenStream {
        let borrowed = self.param_ty(scope, ty, mode).to_string();
        let owned = self.ty(scope, ty, TypeMode::Owned).to_string();
        if borrowed == owned {
            return value;
        }
        match ty {
            Type::String => quote!(::std::string::String::from(#value)),
            Type::Id(id) => {
                let def = &self.resolve.types[*id];
                if def.name.is_some() {
                    return quote!(::core::clone::Clone::clone(#value));
                }
                match &def.kind {
                    TypeDefKind::List(_) => quote!(<[_]>::to_vec(#value)),
                    TypeDefKind::Option(payload) => {
                        let inner = self.to_owned(scope, payload, mode, quote!(value));
                        quote!(::core::option::Option::map(#value, |value| #inner))
                    }
                    TypeDefKind::Result(result) => {
                        let ok = match &result.ok {
                            Some(ok) => self.to_owned(scope, ok, mode, quote!(value)),
                            None => quote!(value),
                        };
                        let err = match &result.err {
                            Some(err) => self.to_owned(scope, err, mode, quote!(value)),
                            None => quote!(value),
                        };
                        quote! {
                            match #value {
                                ::core::result::Result::Ok(value) => ::core::result::Result::Ok(#ok),
                                ::core::result::Result::Err(value) => ::core::result::Result::Err(#err),
                            }
                        }
                    }
                    TypeDefKind::Tuple(tuple) => {
                        let names = (0..tuple.types.len())
                            .map(|index| format_ident!("member{index}"))
                            .collect::<Vec<_>>();
                        let members = tuple
                            .types
                            .iter()
                            .zip(&names)
                            .map(|(ty, name)| self.to_owned(scope, ty, mode, quote!(#name)));
                        quote!({
                            let (#(#names,)*) = #value;
                            (#(#members,)*)
                        })
                    }
                    TypeDefKind::Type(alias) => self.to_owned(scope, alias, mode, value),
                    _ => value,
                }
            }
            _ => value,
        }
    }

    /// The `TypedFunc` type of the export `func`.
    pub fn typed_func(&self, scope: &[String], func: &Function) -> TokenStream {
        let c = &self.c;
        let params = func
            .params
            .iter()
            .map(|param| self.ty(scope, &param.ty, TypeMode::Owned));
        let results = func
            .result
            .iter()
            .map(|ty| self.ty(scope, ty, TypeMode::Owned));
        quote!(#c::TypedFunc<(#(#params,)*), (#(#results,)*)>)
    }

    /// The field, the acquisition, and the methods of the exported
    /// `func`, written in the module `scope`. `key` names the
    /// exported interface, or is `None` for a function of the world.
    /// The acquisition calls `lookup`, a closure the caller binds that
    /// finds a function by its WIT name. `via` reaches the struct that
    /// keeps the field from `self`: `self` itself, or `self.funcs` for
    /// the calls of an exported resource.
    pub fn exported_function(
        &self,
        scope: &[String],
        key: Option<&WorldKey>,
        func: &Function,
        via: TokenStream,
    ) -> ExportedFunction {
        let c = &self.c;
        let flags = self.export_flags(key, func);
        let field_name = ident(&func_field_name(self.resolve, func));
        let typed = self.typed_func(scope, func);
        let wit_name = &func.name;
        let item = func.item_name().to_snake_case();
        let func_name = format_ident!("func_{}", item);
        let call_name = format_ident!("call_{}", item);
        let docs = Self::rustdoc(&func.docs);
        // An export with the `async` and `store` flags, which every
        // async function has, takes an accessor and owned parameters,
        // as Wasmtime's macro writes it, and calls `call_concurrent`.
        let concurrent = flags.contains(FunctionFlags::ASYNC | FunctionFlags::STORE);
        let mode = if concurrent {
            TypeMode::Owned
        } else {
            TypeMode::Borrowed
        };
        let args = (0..func.params.len())
            .map(|index| format_ident!("arg{index}"))
            .collect::<Vec<_>>();
        let params = func.params.iter().zip(&args).map(|(param, arg)| {
            let ty = self.param_ty(scope, &param.ty, mode);
            quote!(#arg: #ty)
        });
        let owned = func
            .params
            .iter()
            .zip(&args)
            .map(|(param, arg)| self.to_owned(scope, &param.ty, mode, quote!(#arg)));
        let result = self.optional_ty(scope, func.result.as_ref(), TypeMode::Owned);
        let (returned, value) = match func.result {
            Some(_) => (quote!((ret0,)), quote!(ret0)),
            None => (quote!(()), quote!(())),
        };
        let (instrument, prelude) = self.export_tracing(key, func, flags);

        let call = if concurrent {
            quote! {
                pub async fn #call_name(
                    &self,
                    accessor: impl #c::AsAccessor,
                    #(#params),*
                ) -> #c::Result<#result> {
                    #prelude
                    let #returned = #via.#field_name
                        .call_concurrent(accessor, (#(#owned,)*))
                        #instrument
                        .await?;
                    #c::Result::Ok(#value)
                }
            }
        } else {
            quote! {
                pub async fn #call_name<_T: 'static>(
                    &self,
                    store: &mut #c::Store<_T>,
                    #(#params),*
                ) -> #c::Result<#result> {
                    #prelude
                    let #returned = #via.#field_name
                        .call(store, (#(#owned,)*))
                        #instrument
                        .await?;
                    #c::Result::Ok(#value)
                }
            }
        };
        let methods = quote! {
            /// The typed function of this export, acquired when the
            /// bindings were made.
            pub fn #func_name(&self) -> &#typed {
                &#via.#field_name
            }

            #docs
            #call
        };
        ExportedFunction {
            field: quote!(#field_name: #typed),
            acquire: quote!(#field_name: lookup(#wit_name)?.typed()?),
            methods,
        }
    }

    /// The span the `tracing` flag puts around an export's call, as
    /// Wasmtime's macro opens it.
    fn export_tracing(
        &self,
        key: Option<&WorldKey>,
        func: &Function,
        flags: FunctionFlags,
    ) -> (TokenStream, TokenStream) {
        if !flags.contains(FunctionFlags::TRACING) {
            return Default::default();
        }
        let c = &self.c;
        let tracing = quote!(#c::__private::tracing);
        let module = match key {
            Some(key) => self.resolve.name_world_key(key),
            None => "default".to_owned(),
        };
        let function = &func.name;
        (
            quote!(.instrument(span)),
            quote! {
                use #tracing::Instrument as _;
                let span = #tracing::span!(
                    #tracing::Level::TRACE,
                    "wit-bindgen export",
                    module = #module,
                    function = #function,
                );
            },
        )
    }
}
