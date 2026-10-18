// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The traits and the registrations of the functions a world imports.

use heck::ToSnakeCase;
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use wit_parser::{Function, Type, TypeDefKind, WorldKey};

use crate::function_flags::FunctionFlags;
use crate::generator::Generator;
use crate::names::{ident, rust_function_name, to_rust_ident};
use crate::trait_extras::TraitExtras;
use crate::type_mode::TypeMode;

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
                    "the `async` flag on the plain import `{}` is not supported: {}",
                    func.name,
                    crate::ledger::ASYNC_IMPORT
                ),
            ));
        }
        Ok(flags)
    }

    /// The flags of the destructor of the imported resource `resource`,
    /// refusing `async`: a destructor is synchronous in the polyfill.
    pub fn drop_flags(&self, key: Option<&WorldKey>, resource: &str) -> syn::Result<FunctionFlags> {
        let flags = self
            .config
            .options
            .imports
            .resource_drop_flags(self.resolve, key, resource);
        if flags.contains(FunctionFlags::ASYNC) {
            return Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "the `async` flag on the `[drop]{resource}` rule is not supported: {}",
                    crate::ledger::ASYNC_DROP
                ),
            ));
        }
        Ok(flags)
    }

    /// The resource the interface `key` names `name`, or the world when
    /// `key` is `None`.
    pub fn resource_id(&self, key: Option<&WorldKey>, name: &str) -> Option<wit_parser::TypeId> {
        match key {
            Some(WorldKey::Interface(id)) => self.resolve.interfaces[*id].types.get(name).copied(),
            Some(WorldKey::Name(_)) => self
                .interface_of(key?)
                .and_then(|id| self.resolve.interfaces[id].types.get(name).copied()),
            None => self.resolve.worlds[self.world]
                .imports
                .get(&WorldKey::Name(name.to_owned()))
                .and_then(|item| match item {
                    wit_parser::WorldItem::Type { id, .. } => Some(*id),
                    _ => None,
                }),
        }
    }

    /// The interface a world's item `key` names.
    fn interface_of(&self, key: &WorldKey) -> Option<wit_parser::InterfaceId> {
        let world = &self.resolve.worlds[self.world];
        match world.imports.get(key).or_else(|| world.exports.get(key))? {
            wit_parser::WorldItem::Interface { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// The constructor, the methods, and the static functions of the
    /// resource `id`, in the order its owner declares them.
    pub fn resource_functions(&self, id: wit_parser::TypeId) -> Vec<&Function> {
        match self.resolve.types[id].owner {
            wit_parser::TypeOwner::Interface(owner) => self.resolve.interfaces[owner]
                .functions
                .values()
                .filter(|func| func.kind.resource() == Some(id))
                .collect(),
            wit_parser::TypeOwner::World(world) => self.resolve.worlds[world]
                .imports
                .values()
                .filter_map(|item| match item {
                    wit_parser::WorldItem::Function(func) => Some(func),
                    _ => None,
                })
                .filter(|func| func.kind.resource() == Some(id))
                .collect(),
            wit_parser::TypeOwner::None => Vec::new(),
        }
    }

    /// The registration of the imported resource `name` on `inst`: its
    /// identity, `ResourceType::host::<R>()`, and a destructor that
    /// calls the trait's `drop`.
    pub fn resource_registration(
        &self,
        key: Option<&WorldKey>,
        name: &str,
    ) -> syn::Result<TokenStream> {
        let c = &self.c;
        let flags = self.drop_flags(key, name)?;
        let camel = heck::ToUpperCamelCase::to_upper_camel_case(name);
        let resource = ident(&camel);
        let host = format_ident!("Host{camel}");
        let with_store = format_ident!("Host{camel}WithStore");
        let call = if flags.contains(FunctionFlags::STORE) {
            quote!(<_D as #with_store<_T>>::drop(#c::Access::<_T, _D>::new(store, host_getter), resource))
        } else {
            quote!(#host::drop(&mut host_getter(store.data_mut()), resource))
        };
        let result = if self.config.options.anyhow {
            quote!(#call.map_err(#c::Error::from))
        } else {
            call
        };
        Ok(quote! {
            inst.resource(
                #name,
                #c::ResourceType::host::<#resource>(),
                move |mut store: #c::StoreContext<'_, _T>, rep: u32| -> #c::Result<()> {
                    let resource = #c::Resource::new_own(rep);
                    let _ = &mut store;
                    #result
                },
            )?;
        })
    }

    /// The traits of the imported `functions`, written in the module
    /// `scope`: `{name}` with a method for each function without the
    /// `store` flag, and `{name}WithStore<T>` with one for each
    /// function with it, as Wasmtime's macro writes them.
    ///
    /// `extras` are what the traits have beyond the functions.
    pub fn import_trait(
        &self,
        scope: &[String],
        key: Option<&WorldKey>,
        name: &str,
        functions: &[&Function],
        extras: TraitExtras<'_>,
    ) -> syn::Result<TokenStream> {
        let TraitExtras {
            converts,
            resources,
            drop,
        } = extras;
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
        // The flags of the resources' own functions decide whether the
        // traits that extend theirs must be `Send` as well.
        for resource in resources {
            let Some(id) = self.resource_id(key, resource) else {
                continue;
            };
            for func in self.resource_functions(id) {
                all_flags |= self.import_flags(key, func)?;
            }
        }
        let drop_flags = drop
            .map(|resource| self.drop_flags(key, resource))
            .transpose()?;

        let mut stored_signatures = stored
            .iter()
            .map(|(func, flags)| self.import_signature(scope, func, *flags))
            .collect::<Vec<_>>();
        let mut plain_signatures = plain
            .iter()
            .map(|(func, flags)| self.import_signature(scope, func, *flags))
            .collect::<Vec<_>>();
        let mut drop_forwarding = None;
        if let (Some(resource), Some(flags)) = (drop, drop_flags) {
            let camel = ident(&heck::ToUpperCamelCase::to_upper_camel_case(resource));
            let result = self.result_path();
            if flags.contains(FunctionFlags::STORE) {
                stored_signatures.push(quote! {
                    /// Drop the resource `rep`, which the guest dropped.
                    fn drop(host: #c::Access<'_, _T, Self>, rep: #c::Resource<#camel>) -> #result<()>
                });
            } else {
                plain_signatures.push(quote! {
                    /// Drop the resource `rep`, which the guest dropped.
                    fn drop(&mut self, rep: #c::Resource<#camel>) -> #result<()>
                });
                drop_forwarding = Some(quote! {
                    fn drop(&mut self, rep: #c::Resource<#camel>) -> #result<()> {
                        #host::drop(*self, rep)
                    }
                });
            }
        }
        let send_bound = all_flags
            .contains(FunctionFlags::ASYNC)
            .then(|| quote!(+ #c::MaybeSend));
        let convert_signatures = converts
            .iter()
            .map(|(name, ty)| self.convert_signature(scope, name, *ty))
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
            let converts =
                converts
                    .iter()
                    .zip(&convert_signatures)
                    .map(|((name, _), signature)| {
                        let method = format_ident!("convert_{}", name.to_snake_case());
                        quote! {
                            #signature {
                                #host::#method(*self, err)
                            }
                        }
                    });
            quote! {
                impl<_T: #host + ?Sized #send_bound> #host for &mut _T {
                    #(#methods)*
                    #(#converts)*
                    #drop_forwarding
                }
            }
        });

        // A trait with an async function is `Send` natively, as
        // Wasmtime's is, so its futures can be; in the browser the
        // marker asks nothing.
        let mut with_store_supertraits = vec![quote!(#c::HasData)];
        let mut host_supertraits = Vec::new();
        for resource in resources {
            let camel = heck::ToUpperCamelCase::to_upper_camel_case(resource.as_str());
            let host_resource = format_ident!("Host{camel}");
            let with_store_resource = format_ident!("Host{camel}WithStore");
            host_supertraits.push(quote!(#host_resource));
            with_store_supertraits.push(quote!(#with_store_resource<_T>));
        }
        if all_flags.contains(FunctionFlags::ASYNC) {
            with_store_supertraits.push(quote!(#c::MaybeSend));
            host_supertraits.push(quote!(#c::MaybeSend));
        }
        // With no method of its own, every `HasData` type that has the
        // supertraits implements `{name}WithStore<T>`, so a host need
        // not write the empty impl.
        let blanket = stored_signatures.is_empty().then(|| {
            quote! {
                impl<_H: ?Sized, _T> #with_store<_T> for _H
                where
                    _H: #(#with_store_supertraits)+*
                {}
            }
        });
        let tokens = quote! {
            pub trait #with_store<_T>: #(#with_store_supertraits)+* {
                #(#stored_signatures;)*
            }
            #blanket
            pub trait #host: #(#host_supertraits)+* {
                #(#plain_signatures;)*
                #(#convert_signatures;)*
            }
            #forwarding
        };
        Ok(tokens)
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
        let receiver = if func.kind.is_async() {
            quote!(accessor: &#c::Accessor<_T, Self>,)
        } else if flags.contains(FunctionFlags::STORE) {
            quote!(host: #c::Access<'_, _T, Self>,)
        } else {
            quote!(&mut self,)
        };
        let params = func.params.iter().map(|param| {
            let name = ident(&to_rust_ident(&param.name));
            let ty = self.ty(scope, &param.ty, TypeMode::Owned);
            quote!(#name: #ty)
        });
        let result = self.import_result(scope, func, flags);
        let result = if func.kind.is_async() {
            quote!(impl ::core::future::Future<Output = #result> + #c::MaybeSend)
        } else {
            result
        };
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
        if flags.contains(FunctionFlags::TRAPPABLE)
            && let Some(trappable) = self.trappable_return(scope, func)
        {
            // A function that returns a `result` whose error type a
            // `trappable_error_type` entry maps returns the mapped type,
            // which `convert_*` turns into the WIT error or a trap.
            let ok = self.optional_ty(scope, trappable.ok.as_ref(), TypeMode::Owned);
            let error = trappable.rust;
            return quote!(::core::result::Result<#ok, #error>);
        }
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
        let span_only = self.import_span(key, func, flags);

        let (host, call) = if flags.contains(FunctionFlags::STORE) {
            let with_store = format_ident!("{host_trait}WithStore");
            (
                quote! {
                    let host = #c::Access::<_T, _D>::new(
                        #c::__private::reborrow(caller.store()),
                        host_getter,
                    );
                },
                quote!(<_D as #with_store<_T>>::#method(host, #(#args),*)),
            )
        } else {
            let host_trait = ident(host_trait);
            (
                quote!(let mut host = host_getter(caller.data_mut());),
                quote!(#host_trait::#method(&mut host, #(#args),*)),
            )
        };
        // The error a `trappable` method returns: passed on as it is, or
        // through `anyhow`, or through the `convert_*` of the mapped
        // error type it returns, whose `Ok` the guest receives.
        let passed_on = if self.config.options.anyhow {
            quote!(.map_err(#c::Error::from)?)
        } else {
            quote!(?)
        };
        let value = if !flags.contains(FunctionFlags::TRAPPABLE) {
            quote!(r)
        } else if let Some(trappable) = self.trappable_return(scope, func) {
            let view = if func.kind.is_async() {
                quote!(&mut access.get())
            } else if flags.contains(FunctionFlags::STORE) {
                quote!(&mut host_getter(caller.data_mut()))
            } else {
                quote!(&mut host)
            };
            let convert = self.convert_call(scope, trappable.error, view)?;
            let converted = if func.kind.is_async() {
                quote!(host.try_with(|mut access| #convert)?#passed_on)
            } else {
                quote!(#convert #passed_on)
            };
            quote! {
                match r {
                    ::core::result::Result::Ok(value) => ::core::result::Result::Ok(value),
                    ::core::result::Result::Err(e) => ::core::result::Result::Err(#converted),
                }
            }
        } else {
            quote!(r #passed_on)
        };
        let results = match func.result {
            Some(_) => quote!((#value,)),
            None => quote!(#value),
        };
        if func.kind.is_async() {
            let with_store = format_ident!("{host_trait}WithStore");
            let instrument = flags
                .contains(FunctionFlags::TRACING)
                .then(|| quote!(.instrument(span)));
            let span = if flags.contains(FunctionFlags::TRACING) {
                let tracing = quote!(#c::__private::tracing);
                quote! {
                    use #tracing::Instrument as _;
                    let span = {
                        #span_only
                        span
                    };
                }
            } else {
                TokenStream::new()
            };
            return Ok(quote! {
                inst.func_wrap_concurrent(
                    #wit_name,
                    move |caller: &#c::Accessor<_T>, (#(#args,)*): (#(#arg_types,)*)| {
                        #span
                        let host = caller.with_getter::<_D>(host_getter);
                        async move {
                            #event
                            let r = <_D as #with_store<_T>>::#method(&host, #(#args),*).await;
                            #returned
                            #c::Result::Ok(#results)
                        }
                        #instrument
                    },
                )?;
            });
        }
        Ok(quote! {
            inst.func_wrap(
                #wit_name,
                move |mut caller: #c::HostCall<'_, _T>, (#(#args,)*): (#(#arg_types,)*)| {
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
        let span = self.import_span(key, func, flags);
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
                #span
                let _enter = span.enter();
            },
            quote!(#tracing::event!(#tracing::Level::TRACE, #(#fields,)* "call");),
            quote!(#tracing::event!(#tracing::Level::TRACE, #result, "return");),
        )
    }

    /// The span the `tracing` flag opens around an import, bound as
    /// `span`, without entering it: a future enters it on each poll.
    fn import_span(
        &self,
        key: Option<&WorldKey>,
        func: &Function,
        flags: FunctionFlags,
    ) -> TokenStream {
        if !flags.contains(FunctionFlags::TRACING) {
            return TokenStream::new();
        }
        let c = &self.c;
        let tracing = quote!(#c::__private::tracing);
        let module = self.tracing_module(key);
        let function = &func.name;
        quote! {
            let span = #tracing::span!(
                #tracing::Level::TRACE,
                "wit-bindgen import",
                module = #module,
                function = #function,
            );
        }
    }

    /// The `module` a tracing span names: the interface, or the world
    /// for a function of the world.
    fn tracing_module(&self, key: Option<&WorldKey>) -> String {
        match key {
            Some(WorldKey::Name(name)) => name.clone(),
            Some(WorldKey::Interface(id)) => self.resolve.interfaces[*id]
                .name
                .clone()
                .unwrap_or_else(|| "<no module>".to_owned()),
            None => self.resolve.worlds[self.world].name.clone(),
        }
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
