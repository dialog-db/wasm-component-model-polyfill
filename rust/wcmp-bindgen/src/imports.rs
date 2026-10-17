// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The traits and the registrations of the functions a world imports.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use wit_parser::{Function, Type, TypeDefKind, WorldKey};

use crate::function_flags::FunctionFlags;
use crate::generator::Generator;
use crate::names::{ident, rust_function_name, to_rust_ident};
use crate::type_mode::TypeMode;

/// The traits one set of imported functions generates: `{name}` and
/// `{name}WithStore<T>`, and the flags of every function in them.
pub struct ImportTrait {
    /// The code of the two traits.
    pub tokens: TokenStream,
    /// The flags of every function in the traits, together.
    pub all_flags: FunctionFlags,
}

impl Generator<'_> {
    /// The flags the `imports` option gives `func`, in the interface
    /// `key` names or at the world's level, refusing the flags the
    /// polyfill does not serve.
    pub fn import_flags(
        &self,
        key: Option<&WorldKey>,
        func: &Function,
    ) -> syn::Result<FunctionFlags> {
        let flags = self.config.options.imports.flags(self.resolve, key, func);
        if flags.contains(FunctionFlags::ASYNC) && !func.kind.is_async() {
            return Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "the `async` flag on the plain import `{}` is not supported: the polyfill \
                     has no host function that holds the store across an await, which \
                     Wasmtime's `func_wrap_async` is; declare the function `async func` in the \
                     WIT to serve it concurrently, or drop the flag to serve it synchronously",
                    func.name
                ),
            ));
        }
        Ok(flags)
    }

    /// The traits of the imported `functions`, written in the module
    /// `scope`: `{name}` with a method for each function without the
    /// `store` flag, and `{name}WithStore<T>` with one for each
    /// function with it, as Wasmtime's macro writes them.
    pub fn import_trait(
        &self,
        scope: &[String],
        key: Option<&WorldKey>,
        name: &str,
        functions: &[&Function],
    ) -> syn::Result<ImportTrait> {
        let c = &self.c;
        let host = ident(name);
        let with_store = format_ident!("{name}WithStore");
        let mut all_flags = FunctionFlags::empty();
        let mut plain = Vec::new();
        let mut stored = Vec::new();
        for func in functions {
            let flags = self.import_flags(key, func)?;
            all_flags |= flags;
            if flags.contains(FunctionFlags::STORE) {
                stored.push((*func, flags));
            } else {
                plain.push((*func, flags));
            }
        }

        let stored_signatures = stored
            .iter()
            .map(|(func, flags)| self.import_signature(scope, func, *flags))
            .collect::<Vec<_>>();
        let blanket = stored.is_empty().then(|| {
            quote! {
                impl<_H: ?Sized, T> #with_store<T> for _H where _H: #c::HasData {}
            }
        });
        let plain_signatures = plain
            .iter()
            .map(|(func, flags)| self.import_signature(scope, func, *flags))
            .collect::<Vec<_>>();
        let forwarding = (!self.config.options.skip_mut_forwarding_impls).then(|| {
            let methods = plain
                .iter()
                .zip(&plain_signatures)
                .map(|((func, _), signature)| {
                    let method = ident(&rust_function_name(func));
                    let args = func
                        .params
                        .iter()
                        .map(|param| ident(&to_rust_ident(&param.name)));
                    quote! {
                        #signature {
                            #host::#method(*self, #(#args),*)
                        }
                    }
                });
            quote! {
                impl<_T: #host + ?Sized> #host for &mut _T {
                    #(#methods)*
                }
            }
        });

        let tokens = quote! {
            pub trait #with_store<T>: #c::HasData {
                #(#stored_signatures;)*
            }
            #blanket
            pub trait #host {
                #(#plain_signatures;)*
            }
            #forwarding
        };
        Ok(ImportTrait { tokens, all_flags })
    }

    /// The trait method of the imported `func`, without its body.
    fn import_signature(
        &self,
        scope: &[String],
        func: &Function,
        flags: FunctionFlags,
    ) -> TokenStream {
        let c = &self.c;
        let docs = Self::rustdoc(&func.docs);
        let method = ident(&rust_function_name(func));
        let receiver = if flags.contains(FunctionFlags::STORE) {
            quote!(host: #c::Access<'_, T, Self>,)
        } else {
            quote!(&mut self,)
        };
        let params = func.params.iter().map(|param| {
            let name = ident(&to_rust_ident(&param.name));
            let ty = self.ty(scope, &param.ty, TypeMode::Owned);
            quote!(#name: #ty)
        });
        let result = self.import_result(scope, func, flags);
        quote! {
            #docs
            fn #method(#receiver #(#params),*) -> #result
        }
    }

    /// The return type of the trait method of the imported `func`.
    fn import_result(
        &self,
        scope: &[String],
        func: &Function,
        flags: FunctionFlags,
    ) -> TokenStream {
        let result = self.optional_ty(scope, func.result.as_ref(), TypeMode::Owned);
        if flags.contains(FunctionFlags::TRAPPABLE) {
            let wrapper = self.result_path();
            quote!(#wrapper<#result>)
        } else {
            result
        }
    }

    /// The `Result` a trappable method returns: the polyfill's, or
    /// `anyhow`'s under `anyhow: true`.
    pub fn result_path(&self) -> TokenStream {
        let c = &self.c;
        if self.config.options.anyhow {
            quote!(#c::__private::anyhow::Result)
        } else {
            quote!(#c::Result)
        }
    }

    /// The registration of the imported `func` on the linker instance
    /// `inst`, whose method is in the trait `host_trait`.
    pub fn import_registration(
        &self,
        scope: &[String],
        key: Option<&WorldKey>,
        func: &Function,
        host_trait: &str,
    ) -> syn::Result<TokenStream> {
        let c = &self.c;
        let flags = self.import_flags(key, func)?;
        let wit_name = &func.name;
        let method = ident(&rust_function_name(func));
        let args = (0..func.params.len())
            .map(|index| format_ident!("arg{index}"))
            .collect::<Vec<_>>();
        let arg_types = func
            .params
            .iter()
            .map(|param| self.ty(scope, &param.ty, TypeMode::Owned))
            .collect::<Vec<_>>();
        let (span, event, returned) = self.import_tracing(key, func, flags, &args);

        let (host, call) = if flags.contains(FunctionFlags::STORE) {
            let with_store = format_ident!("{host_trait}WithStore");
            (
                quote!(let host = #c::Access::<T, D>::new(caller.into_store(), host_getter);),
                quote!(<D as #with_store<T>>::#method(host, #(#args),*)),
            )
        } else {
            let host_trait = ident(host_trait);
            (
                quote!(let host = &mut host_getter(caller.data_mut());),
                quote!(#host_trait::#method(host, #(#args),*)),
            )
        };
        let value = if !flags.contains(FunctionFlags::TRAPPABLE) {
            quote!(r)
        } else if self.config.options.anyhow {
            quote!(r.map_err(#c::Error::from)?)
        } else {
            quote!(r?)
        };
        let results = match func.result {
            Some(_) => quote!((#value,)),
            None => quote!(#value),
        };
        let mutable = (!flags.contains(FunctionFlags::STORE)).then(|| quote!(mut));
        Ok(quote! {
            inst.func_wrap(
                #wit_name,
                move |#mutable caller: #c::HostCall<'_, T>, (#(#args,)*): (#(#arg_types,)*)| {
                    #span
                    #event
                    #host
                    let r = #call;
                    #returned
                    #c::Result::Ok(#results)
                },
            )?;
        })
    }

    /// The span and the events the `tracing` and `verbose_tracing` flags
    /// add to a registration, as Wasmtime's macro records them: the
    /// arguments on the call, the result on the return, and a list in
    /// full only under `verbose_tracing`.
    fn import_tracing(
        &self,
        key: Option<&WorldKey>,
        func: &Function,
        flags: FunctionFlags,
        args: &[syn::Ident],
    ) -> (TokenStream, TokenStream, TokenStream) {
        if !flags.contains(FunctionFlags::TRACING) {
            return Default::default();
        }
        let c = &self.c;
        let tracing = quote!(#c::__private::tracing);
        let module = match key {
            Some(WorldKey::Name(name)) => name.clone(),
            Some(WorldKey::Interface(id)) => self.resolve.interfaces[*id]
                .name
                .clone()
                .unwrap_or_else(|| "<no module>".to_owned()),
            None => self.resolve.worlds[self.world].name.clone(),
        };
        let function = &func.name;
        let verbose = flags.contains(FunctionFlags::VERBOSE_TRACING);
        let fields = func.params.iter().zip(args).map(|(param, arg)| {
            let name = ident(&to_rust_ident(&param.name));
            if !verbose && self.contains_lists(&param.ty) {
                quote!(#name = #tracing::field::debug("..."))
            } else {
                quote!(#name = #tracing::field::debug(&#arg))
            }
        });
        let result = match func.result {
            Some(ty) if !verbose && self.contains_lists(&ty) => {
                quote!(result = #tracing::field::debug("..."))
            }
            _ => quote!(result = #tracing::field::debug(&r)),
        };
        (
            quote! {
                let span = #tracing::span!(
                    #tracing::Level::TRACE,
                    "wit-bindgen import",
                    module = #module,
                    function = #function,
                );
                let _enter = span.enter();
            },
            quote!(#tracing::event!(#tracing::Level::TRACE, #(#fields,)* "call");),
            quote!(#tracing::event!(#tracing::Level::TRACE, #result, "return");),
        )
    }

    /// Whether `ty` holds a list, which tracing records in full only
    /// under `verbose_tracing`. A string is not a list here.
    pub fn contains_lists(&self, ty: &Type) -> bool {
        let Type::Id(id) = ty else {
            return false;
        };
        let optional = |ty: &Option<Type>| ty.as_ref().is_some_and(|ty| self.contains_lists(ty));
        match &self.resolve.types[*id].kind {
            TypeDefKind::List(_) => true,
            TypeDefKind::Map(key, value) => self.contains_lists(key) || self.contains_lists(value),
            TypeDefKind::Option(ty)
            | TypeDefKind::Type(ty)
            | TypeDefKind::FixedLengthList(ty, _) => self.contains_lists(ty),
            TypeDefKind::Result(result) => optional(&result.ok) || optional(&result.err),
            TypeDefKind::Record(record) => record
                .fields
                .iter()
                .any(|field| self.contains_lists(&field.ty)),
            TypeDefKind::Tuple(tuple) => tuple.types.iter().any(|ty| self.contains_lists(ty)),
            TypeDefKind::Variant(variant) => variant.cases.iter().any(|case| optional(&case.ty)),
            TypeDefKind::Resource
            | TypeDefKind::Handle(_)
            | TypeDefKind::Flags(_)
            | TypeDefKind::Enum(_)
            | TypeDefKind::Future(_)
            | TypeDefKind::Stream(_)
            | TypeDefKind::Unknown => false,
        }
    }
}
