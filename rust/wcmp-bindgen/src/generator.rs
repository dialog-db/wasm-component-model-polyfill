// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The code one invocation writes.

use std::collections::{BTreeSet, HashMap};

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use wit_parser::{
    Docs, Enum, Flags, Handle, InterfaceId, PackageId, Record, Resolve, SizeAlign, Type,
    TypeDefKind, TypeId, TypeOwner, Variant, WorldId, WorldItem, WorldKey,
};

use crate::config::Config;
use crate::direction::Direction;
use crate::interface_name::InterfaceName;
use crate::module_tree::ModuleTree;
use crate::names::{ident, resolve_type_definition_id, to_rust_ident, to_rust_upper_camel_case};
use crate::trait_extras::TraitExtras;
use crate::type_info::TypeInfos;
use crate::type_mode::TypeMode;
use crate::with::WithName;

/// The code one invocation writes: the generator over its `Resolve`.
pub struct Generator<'a> {
    /// The invocation.
    pub config: &'a Config,
    /// The WIT.
    pub resolve: &'a Resolve,
    /// The world the invocation generates.
    pub world: WorldId,
    /// The path of the polyfill crate: `::wcmp`, or `wcmp_crate`.
    pub c: TokenStream,
    /// The sizes and alignments of the WIT types.
    pub sizes: SizeAlign,
    /// What the generator knows of each type.
    pub infos: TypeInfos,
    /// Where each imported or exported interface's code is.
    pub interface_names: HashMap<InterfaceId, (InterfaceName, Direction)>,
    /// The modules written so far.
    pub modules: ModuleTree,
    /// Each imported interface, by its name in the world and its
    /// module path, in the world's order.
    pub imported_interfaces: Vec<(WorldKey, Vec<String>)>,
    /// Each exported interface, by its name in the world, its id, and
    /// its module path, in the world's order.
    pub exported_interfaces: Vec<(WorldKey, InterfaceId, Vec<String>)>,
    /// The alias each WIT error type a `trappable_error_type` entry maps
    /// is named by at the root.
    pub trappable_errors: HashMap<TypeId, syn::Ident>,
    /// The `with` entries, by the aliases the root names them by.
    pub with_names: Vec<WithName>,
}

impl<'a> Generator<'a> {
    /// A generator for the world `config` selects.
    pub fn new(config: &'a Config) -> Self {
        let resolve = &config.resolve;
        let mut sizes = SizeAlign::default();
        sizes.fill(resolve);
        let c = match &config.options.wcmp_crate {
            Some(path) => quote!(#path),
            None => quote!(::wcmp),
        };
        Self {
            config,
            resolve,
            world: config.world,
            c,
            sizes,
            infos: TypeInfos::analyze(resolve, config.world),
            interface_names: HashMap::new(),
            modules: ModuleTree::default(),
            imported_interfaces: Vec::new(),
            exported_interfaces: Vec::new(),
            trappable_errors: HashMap::new(),
            with_names: Vec::new(),
        }
    }

    /// Write the code.
    pub fn generate(mut self) -> syn::Result<TokenStream> {
        let world = &self.resolve.worlds[self.world];
        let only_interfaces = self.config.options.only_interfaces;
        self.refuse_options()?;
        let mut root = self.with_aliases();
        root.extend(self.resolve_trappable_errors()?);
        let mut world_functions = Vec::new();

        for (key, item) in world.imports.iter() {
            match item {
                WorldItem::Interface { id, .. } => self.import_interface(key, *id)?,
                WorldItem::Type { id, .. } if !only_interfaces => {
                    let name = match key {
                        WorldKey::Name(name) => name.clone(),
                        WorldKey::Interface(_) => continue,
                    };
                    root.extend(self.define_type(&[], &name, *id));
                }
                WorldItem::Function(func) if !only_interfaces => {
                    if func.kind.resource().is_none() {
                        world_functions.push(func);
                    }
                }
                WorldItem::Function(_) | WorldItem::Type { .. } => {}
            }
        }
        root.extend(self.world_imports(&world_functions)?);
        for (key, item) in world.exports.iter() {
            if let WorldItem::Interface { id, .. } = item {
                self.export_interface(key, *id)?;
            }
        }

        if !only_interfaces {
            root.extend(self.world_struct(&world_functions)?);
        }
        self.config.options.imports.check_all_used("imports")?;
        self.config.options.exports.check_all_used("exports")?;
        self.check_with_used()?;

        let mut tokens = root;
        tokens.extend(std::mem::take(&mut self.modules).into_tokens());
        Ok(tokens)
    }

    /// The module path the interface `id`, named `key` in the world,
    /// is written at, as Wasmtime's macro names it.
    fn interface_path(&self, key: &WorldKey, id: InterfaceId, direction: Direction) -> Vec<String> {
        let mut path = Vec::new();
        if direction == Direction::Export {
            path.push("exports".to_owned());
        }
        match key {
            WorldKey::Name(name) => path.push(to_rust_ident(name)),
            WorldKey::Interface(_) => {
                let interface = &self.resolve.interfaces[id];
                let package = interface
                    .package
                    .expect("an interface with an id has a package");
                path.push(to_rust_ident(
                    &self.resolve.packages[package].name.namespace,
                ));
                path.push(self.package_module(package));
                path.push(to_rust_ident(interface.name.as_deref().unwrap_or_default()));
            }
        }
        path
    }

    /// The module name of the package `id`: its name, and its version
    /// when another package of the same name has another version, as
    /// `dep0_1_0`.
    fn package_module(&self, id: PackageId) -> String {
        let package = &self.resolve.packages[id].name;
        // A name that is a Rust keyword is escaped as any other is.
        let base = to_rust_ident(&package.name);
        let same_name = self
            .resolve
            .packages
            .iter()
            .filter(|(_, other)| {
                other.name.namespace == package.namespace && other.name.name == package.name
            })
            .count();
        match &package.version {
            Some(version) if same_name > 1 => {
                let version = version
                    .to_string()
                    .replace(['.', '-', '+'], "_")
                    .to_snake_case();
                format!("{base}{version}")
            }
            _ => base,
        }
    }

    /// Write the module of the imported interface `id`.
    fn import_interface(&mut self, key: &WorldKey, id: InterfaceId) -> syn::Result<()> {
        let c = self.c.clone();
        let path = self.interface_path(key, id, Direction::Import);
        self.interface_names
            .insert(id, (InterfaceName::Path(path.clone()), Direction::Import));
        if self.remap_interface(key, &path) {
            self.imported_interfaces.push((key.clone(), path));
            return Ok(());
        }
        let mut items = self.interface_types(&path, id);

        let interface = &self.resolve.interfaces[id];
        let functions = interface
            .functions
            .values()
            .filter(|func| func.kind.resource().is_none())
            .collect::<Vec<_>>();
        // Each resource the interface defines has traits of its own,
        // which the interface's extend, and a registration with its
        // destructor. A resource the interface `use`s from another is
        // that interface's: it is registered there alone, as in Wasmtime.
        let resources = interface
            .types
            .iter()
            .filter(|(_, ty)| matches!(self.resolve.types[**ty].kind, TypeDefKind::Resource))
            .map(|(name, ty)| (name.clone(), *ty))
            .collect::<Vec<_>>();
        let mut registrations = Vec::new();
        for (name, ty) in &resources {
            let camel = heck::ToUpperCamelCase::to_upper_camel_case(name.as_str());
            let functions = self.resource_functions(*ty);
            let traits = self.import_trait(
                &path,
                Some(key),
                &format!("Host{camel}"),
                &functions,
                TraitExtras {
                    drop: Some(name),
                    ..TraitExtras::default()
                },
            )?;
            items.extend(traits);
            registrations.push(self.resource_registration(Some(key), name)?);
            for func in functions {
                registrations.push(self.import_registration(
                    &path,
                    Some(key),
                    func,
                    &format!("Host{camel}"),
                )?);
            }
        }
        let resource_names = resources
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        let converts = self.interface_error_types(id);
        let host = self.import_trait(
            &path,
            Some(key),
            "Host",
            &functions,
            TraitExtras {
                converts: &converts,
                resources: &resource_names,
                drop: None,
            },
        )?;
        items.extend(host);
        for func in &functions {
            registrations.push(self.import_registration(&path, Some(key), func, "Host")?);
        }
        let bounds = self.store_data_bounds();
        let conversions = self.conversion_traits(&path, id);
        let name = self.resolve.name_world_key(key);
        let instance = match key {
            WorldKey::Name(_) => quote! {
                let mut root = linker.root();
                let mut inst = root.instance(#name);
            },
            WorldKey::Interface(_) => quote! {
                let id = #c::__private::interface_id(#name)?;
                let mut inst = linker.instance(&id);
            },
        };
        items.extend(quote! {
            /// Register the host functions of this interface on `inst`,
            /// each calling the trait method on what `host_getter`
            /// reaches from the host data.
            pub fn add_to_linker_instance<_T, _D>(
                inst: &mut #c::LinkerInstance<'_, _T>,
                host_getter: fn(&mut _T) -> _D::Data<'_>,
            ) -> #c::Result<()>
            where
                _D: HostWithStore<_T>,
                for<'a> _D::Data<'a>: Host #(+ #conversions)*,
                _T: 'static #bounds,
            {
                // An interface of resources or types alone registers no
                // function.
                let _ = (&inst, host_getter);
                #(#registrations)*
                #c::Result::Ok(())
            }

            /// Register the host functions of this interface on the
            /// linker, under the interface's name.
            pub fn add_to_linker<_T, _D>(
                linker: &mut #c::Linker<_T>,
                host_getter: fn(&mut _T) -> _D::Data<'_>,
            ) -> #c::Result<()>
            where
                _D: HostWithStore<_T>,
                for<'a> _D::Data<'a>: Host #(+ #conversions)*,
                _T: 'static #bounds,
            {
                #instance
                add_to_linker_instance::<_T, _D>(&mut inst, host_getter)
            }
        });
        self.modules.insert(&path, items);
        self.imported_interfaces.push((key.clone(), path));
        Ok(())
    }

    /// The bounds `require_store_data_send` adds to the host data `T`:
    /// `Send` natively, through a marker that asks nothing in the
    /// browser, so that the macro writes the same code for both.
    pub fn store_data_bounds(&self) -> TokenStream {
        let c = &self.c;
        if self.config.options.require_store_data_send {
            quote!(+ #c::MaybeSend)
        } else {
            TokenStream::new()
        }
    }

    /// The trait and the registration of the functions the world
    /// imports outside an interface: `{World}Imports`,
    /// `{World}ImportsWithStore<T>`, and `add_to_linker_imports`.
    fn world_imports(&self, functions: &[&wit_parser::Function]) -> syn::Result<TokenStream> {
        let resources = self.world_resources();
        if functions.is_empty() && resources.is_empty() {
            return Ok(TokenStream::new());
        }
        let c = &self.c;
        let world = to_rust_upper_camel_case(&self.resolve.worlds[self.world].name);
        let name = format!("{world}Imports");
        let host = ident(&name);
        let with_store = format_ident!("{name}WithStore");
        let mut tokens = TokenStream::new();
        let mut registrations = Vec::new();
        for (resource, ty) in &resources {
            let camel = heck::ToUpperCamelCase::to_upper_camel_case(resource.as_str());
            let resource_functions = self.resource_functions(*ty);
            let traits = self.import_trait(
                &[],
                None,
                &format!("Host{camel}"),
                &resource_functions,
                TraitExtras {
                    drop: Some(resource),
                    ..TraitExtras::default()
                },
            )?;
            tokens.extend(traits);
            registrations.push(self.resource_registration(None, resource)?);
            for func in resource_functions {
                registrations.push(self.import_registration(
                    &[],
                    None,
                    func,
                    &format!("Host{camel}"),
                )?);
            }
        }
        let resource_names = resources
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let imports = self.import_trait(
            &[],
            None,
            &name,
            functions,
            TraitExtras {
                resources: &resource_names,
                ..TraitExtras::default()
            },
        )?;
        for func in functions {
            registrations.push(self.import_registration(&[], None, func, &name)?);
        }
        let bounds = self.store_data_bounds();
        tokens.extend(imports);
        Ok(quote! {
            #tokens

            /// Register the host functions the world imports outside an
            /// interface on the linker's root.
            pub fn add_to_linker_imports<_T, _D>(
                linker: &mut #c::Linker<_T>,
                host_getter: fn(&mut _T) -> _D::Data<'_>,
            ) -> #c::Result<()>
            where
                _D: #with_store<_T>,
                for<'a> _D::Data<'a>: #host,
                _T: 'static #bounds,
            {
                let mut inst = linker.root();
                #(#registrations)*
                #c::Result::Ok(())
            }
        })
    }

    /// The resources the world imports outside an interface, by their
    /// WIT names.
    pub fn world_resources(&self) -> Vec<(String, TypeId)> {
        self.resolve.worlds[self.world]
            .imports
            .iter()
            .filter_map(|(key, item)| match (key, item) {
                (WorldKey::Name(name), WorldItem::Type { id, .. })
                    if matches!(self.resolve.types[*id].kind, TypeDefKind::Resource) =>
                {
                    Some((name.clone(), *id))
                }
                _ => None,
            })
            .collect()
    }

    /// Write the module of the exported interface `id`.
    fn export_interface(&mut self, key: &WorldKey, id: InterfaceId) -> syn::Result<()> {
        let c = self.c.clone();
        let path = self.interface_path(key, id, Direction::Export);
        self.interface_names
            .insert(id, (InterfaceName::Path(path.clone()), Direction::Export));
        if self.remap_interface(key, &path) {
            self.exported_interfaces.push((key.clone(), id, path));
            return Ok(());
        }
        let mut items = self.interface_types(&path, id);

        let interface = &self.resolve.interfaces[id];
        let functions = interface
            .functions
            .values()
            .filter(|func| func.kind.resource().is_none())
            .map(|func| self.exported_function(&path, Some(key), func, quote!(self)))
            .collect::<Vec<_>>();
        // The calls of each exported resource are fields of `Guest` too,
        // which a getter named after the resource reaches through
        // `Guest{R}`, as Wasmtime's macro writes them.
        let mut resource_fields = Vec::new();
        let mut resource_items = TokenStream::new();
        let mut resource_getters = Vec::new();
        for (resource, ty) in interface
            .types
            .iter()
            .filter(|(_, ty)| matches!(self.resolve.types[**ty].kind, TypeDefKind::Resource))
        {
            let calls = self
                .resource_functions(*ty)
                .into_iter()
                .map(|func| self.exported_function(&path, Some(key), func, quote!(self.funcs)))
                .collect::<Vec<_>>();
            let camel = heck::ToUpperCamelCase::to_upper_camel_case(resource.as_str());
            let guest = format_ident!("Guest{camel}");
            let getter = ident(&to_rust_ident(resource));
            let methods = calls.iter().map(|func| &func.methods);
            resource_items.extend(quote! {
                /// The calls of an exported resource.
                pub struct #guest<'a> {
                    funcs: &'a Guest,
                }

                impl #guest<'_> {
                    #(#methods)*
                }
            });
            resource_getters.push(quote! {
                /// The calls of the exported resource.
                pub fn #getter(&self) -> #guest<'_> {
                    #guest { funcs: self }
                }
            });
            resource_fields.extend(calls);
        }
        let fields = functions
            .iter()
            .chain(&resource_fields)
            .map(|func| &func.field);
        let acquire = functions
            .iter()
            .chain(&resource_fields)
            .map(|func| &func.acquire);
        let methods = functions.iter().map(|func| &func.methods);
        let name = self.resolve.name_world_key(key);
        items.extend(quote! {
            /// The calls of this exported interface, acquired from an
            /// instance with their type checks done.
            pub struct Guest {
                #(#fields,)*
            }

            impl Guest {
                /// Acquire each function of this interface from the
                /// instance's export of it, running each type check.
                #[doc(hidden)]
                pub fn new(instance: &#c::Instance) -> #c::Result<Guest> {
                    let exports = instance.exports();
                    let interface = exports.instance(#name).ok_or_else(|| {
                        #c::Error::MissingExport {
                            name: ::std::string::String::from(#name),
                            instance: ::core::option::Option::None,
                        }
                    })?;
                    let lookup = |func: &str| {
                        interface.func(func).ok_or_else(|| #c::Error::MissingExport {
                            name: ::std::string::String::from(func),
                            instance: ::core::option::Option::Some(::std::string::String::from(#name)),
                        })
                    };
                    let _ = &lookup;
                    #c::Result::Ok(Guest {
                        #(#acquire,)*
                    })
                }

                #(#resource_getters)*

                #(#methods)*
            }

            #resource_items
        });
        self.modules.insert(&path, items);
        self.exported_interfaces.push((key.clone(), id, path));
        Ok(())
    }

    /// The struct of the world: the exports, acquired from an instance,
    /// `instantiate_async`, `new`, `add_to_linker`, and the getters and
    /// calls, as Wasmtime's macro writes it, without `instantiate`,
    /// `{World}Pre`, and `{World}Indices`.
    fn world_struct(&self, world_functions: &[&wit_parser::Function]) -> syn::Result<TokenStream> {
        let c = &self.c;
        let world = &self.resolve.worlds[self.world];
        let name = ident(&to_rust_upper_camel_case(&world.name));
        let world_name = &world.name;

        let mut fields = Vec::new();
        let mut acquire = Vec::new();
        let mut getters = Vec::new();
        for (index, (key, id, path)) in self.exported_interfaces.iter().enumerate() {
            let field = format_ident!("interface{index}");
            let segments = path.iter().map(|segment| ident(segment));
            let ty = quote!(#(#segments)::*::Guest);
            fields.push(quote!(#field: #ty));
            acquire.push(quote!(#field: #ty::new(instance)?));
            let getter = match key {
                WorldKey::Name(name) => ident(&to_rust_ident(name)),
                WorldKey::Interface(_) => {
                    let interface = &self.resolve.interfaces[*id];
                    let package = interface
                        .package
                        .expect("an interface with an id has a package");
                    format_ident!(
                        "{}_{}_{}",
                        to_rust_ident(&self.resolve.packages[package].name.namespace),
                        self.package_module(package),
                        to_rust_ident(interface.name.as_deref().unwrap_or_default())
                    )
                }
            };
            getters.push(quote! {
                /// The calls of the exported interface.
                pub fn #getter(&self) -> &#ty {
                    &self.#field
                }
            });
        }

        let exports = world
            .exports
            .values()
            .filter_map(|item| match item {
                WorldItem::Function(func) => Some(func),
                _ => None,
            })
            .map(|func| self.exported_function(&[], None, func, quote!(self)))
            .collect::<Vec<_>>();
        fields.extend(exports.iter().map(|func| func.field.clone()));
        acquire.extend(exports.iter().map(|func| func.acquire.clone()));
        let methods = exports.iter().map(|func| &func.methods);

        // `add_to_linker` asks of `D` and its view every trait of every
        // imported interface, and the world's own.
        let mut with_store = vec![quote!(#c::HasData)];
        let mut host = Vec::new();
        let mut registrations = Vec::new();
        for (_, path) in &self.imported_interfaces {
            let segments = path
                .iter()
                .map(|segment| ident(segment))
                .collect::<Vec<_>>();
            with_store.push(quote!(#(#segments)::*::HostWithStore<_T>));
            host.push(quote!(#(#segments)::*::Host));
            registrations
                .push(quote!(#(#segments)::*::add_to_linker::<_T, _D>(linker, host_getter)?;));
        }
        if !world_functions.is_empty() || !self.world_resources().is_empty() {
            let imports = format_ident!("{name}Imports");
            let imports_with_store = format_ident!("{name}ImportsWithStore");
            with_store.push(quote!(#imports_with_store<_T>));
            host.push(quote!(#imports));
            registrations.push(quote!(add_to_linker_imports::<_T, _D>(linker, host_getter)?;));
        }
        let host_bound = (!host.is_empty()).then(|| quote!(for<'a> _D::Data<'a>: #(#host)+*,));
        let bounds = self.store_data_bounds();

        Ok(quote! {
            #[doc = concat!(" The bindings of an instance of a component that implements the world `", #world_name, "`.")]
            pub struct #name {
                #(#fields,)*
            }

            impl #name {
                /// Instantiate `component` into `store` with `linker`,
                /// and acquire the world's exports from the instance.
                pub async fn instantiate_async<_T: 'static>(
                    store: &mut #c::Store<_T>,
                    component: &#c::Component,
                    linker: &#c::Linker<_T>,
                ) -> #c::Result<#name> {
                    let instance = linker.instantiate(store, component).await?;
                    #name::new(store, &instance)
                }

                /// Acquire the world's exports from `instance`, running
                /// the type check of each one here and never in a call.
                ///
                /// `store` is not read: it keeps the signature of
                /// Wasmtime's `new`.
                pub fn new<_T: 'static>(
                    store: &mut #c::Store<_T>,
                    instance: &#c::Instance,
                ) -> #c::Result<#name> {
                    let _ = store;
                    let exports = instance.exports();
                    let lookup = |func: &str| {
                        exports.func(func).ok_or_else(|| #c::Error::MissingExport {
                            name: ::std::string::String::from(func),
                            instance: ::core::option::Option::None,
                        })
                    };
                    let _ = &lookup;
                    #c::Result::Ok(#name {
                        #(#acquire,)*
                    })
                }

                /// Register every import of the world on the linker.
                pub fn add_to_linker<_T, _D>(
                    linker: &mut #c::Linker<_T>,
                    host_getter: fn(&mut _T) -> _D::Data<'_>,
                ) -> #c::Result<()>
                where
                    _D: #(#with_store)+*,
                    #host_bound
                    _T: 'static #bounds,
                {
                    let _ = (&linker, host_getter);
                    #(#registrations)*
                    #c::Result::Ok(())
                }

                #(#getters)*

                #(#methods)*
            }
        })
    }

    /// Refuse the options of the ledger the polyfill does not serve,
    /// naming each and the reason.
    fn refuse_options(&self) -> syn::Result<()> {
        let options = &self.config.options;
        if let crate::ownership::Ownership::Borrowing { span } = options.ownership {
            return Err(syn::Error::new(
                span,
                format!(
                    "`ownership: Borrowing {{ … }}` is not supported: {}",
                    crate::ledger::BORROWING
                ),
            ));
        }
        if let Some(span) = options.named_imports {
            return Err(syn::Error::new(
                span,
                format!(
                    "`named_imports` is not supported: {}",
                    crate::ledger::NAMED_IMPORTS
                ),
            ));
        }
        if let Some(span) = options.include_component_type {
            return Err(syn::Error::new(
                span,
                format!(
                    "`include_component_type` is not supported: {}",
                    crate::ledger::INCLUDE_COMPONENT_TYPE
                ),
            ));
        }
        Ok(())
    }

    /// Write the module at `path` of an interface `with` maps as a
    /// re-export of what it maps to, as Wasmtime's macro does, and
    /// answer whether it did. The macro writes nothing else for the
    /// interface: its traits, its types, and its registrations are the
    /// mapped module's, which must be one `wcmp::bindgen!` wrote.
    fn remap_interface(&mut self, key: &WorldKey, path: &[String]) -> bool {
        let Some(mapped) = self.lookup_replacement(Some(key), None) else {
            return false;
        };
        let root = Self::path_to_root(path);
        self.modules.insert(
            path,
            quote! {
                pub use #root #mapped::*;
            },
        );
        true
    }

    /// The name the world gives the interface `id`, if it names it.
    pub fn world_key_of(&self, id: InterfaceId) -> Option<&WorldKey> {
        let world = &self.resolve.worlds[self.world];
        world
            .imports
            .iter()
            .chain(world.exports.iter())
            .find_map(|(key, item)| match item {
                WorldItem::Interface { id: item, .. } if *item == id => Some(key),
                _ => None,
            })
    }

    /// The types of the interface `id`, written in the module `scope`.
    fn interface_types(&self, scope: &[String], id: InterfaceId) -> TokenStream {
        self.resolve.interfaces[id]
            .types
            .iter()
            .map(|(name, ty)| self.define_type(scope, name, *ty))
            .collect()
    }

    /// The path from the module `scope` to the root of the invocation:
    /// one `super` per module.
    pub fn path_to_root(scope: &[String]) -> TokenStream {
        let supers = scope.iter().map(|_| quote!(super::));
        quote!(#(#supers)*)
    }

    /// The path from the module `scope` to the module of the interface
    /// `id`, or none when `scope` is that module.
    pub fn path_to_interface(&self, scope: &[String], id: InterfaceId) -> Option<TokenStream> {
        let (InterfaceName::Path(path), _) = self.interface_names.get(&id)?;
        if path == scope {
            return None;
        }
        let root = Self::path_to_root(scope);
        let segments = path.iter().map(|segment| ident(segment));
        Some(quote!(#root #(#segments)::*))
    }

    /// Whether the resources the interface `id` defines are the host's:
    /// it is imported, or it is not part of the world at all.
    pub fn is_imported_interface(&self, id: InterfaceId) -> bool {
        !matches!(self.interface_names.get(&id), Some((_, Direction::Export)))
    }

    /// The name of the type `id`, from the module `scope`.
    fn type_path(&self, scope: &[String], id: TypeId) -> TokenStream {
        let ty = &self.resolve.types[id];
        let name = ident(&to_rust_upper_camel_case(
            ty.name.as_deref().unwrap_or_default(),
        ));
        match ty.owner {
            TypeOwner::Interface(owner) => match self.path_to_interface(scope, owner) {
                Some(path) => quote!(#path::#name),
                None => quote!(#name),
            },
            TypeOwner::World(_) | TypeOwner::None => {
                let root = Self::path_to_root(scope);
                quote!(#root #name)
            }
        }
    }

    /// `ty` as a Rust type, from the module `scope`.
    pub fn ty(&self, scope: &[String], ty: &Type, mode: TypeMode) -> TokenStream {
        let c = &self.c;
        match ty {
            Type::Bool => quote!(bool),
            Type::U8 => quote!(u8),
            Type::U16 => quote!(u16),
            Type::U32 => quote!(u32),
            Type::U64 => quote!(u64),
            Type::S8 => quote!(i8),
            Type::S16 => quote!(i16),
            Type::S32 => quote!(i32),
            Type::S64 => quote!(i64),
            Type::F32 => quote!(f32),
            Type::F64 => quote!(f64),
            Type::Char => quote!(char),
            Type::String => match mode {
                TypeMode::Owned => quote!(::std::string::String),
                TypeMode::Borrowed => quote!(&str),
            },
            Type::ErrorContext => quote!(#c::ErrorContext),
            Type::Id(id) => self.type_id(scope, *id, mode),
        }
    }

    /// `ty`, or `()` for no type.
    pub fn optional_ty(&self, scope: &[String], ty: Option<&Type>, mode: TypeMode) -> TokenStream {
        match ty {
            Some(ty) => self.ty(scope, ty, mode),
            None => quote!(()),
        }
    }

    /// The type `id` as a Rust type, from the module `scope`.
    fn type_id(&self, scope: &[String], id: TypeId, mode: TypeMode) -> TokenStream {
        let c = &self.c;
        let ty = &self.resolve.types[id];
        if ty.name.is_some() {
            let path = self.type_path(scope, id);
            // A named type that holds a list is lent to an export
            // rather than moved, as Wasmtime's owning types are.
            return if mode == TypeMode::Borrowed && self.infos.get(id).has_list {
                quote!(&#path)
            } else {
                path
            };
        }
        match &ty.kind {
            TypeDefKind::List(element) => {
                let element = self.ty(scope, element, TypeMode::Owned);
                match mode {
                    TypeMode::Owned => quote!(::std::vec::Vec<#element>),
                    TypeMode::Borrowed => quote!(&[#element]),
                }
            }
            TypeDefKind::Option(payload) => {
                let payload = self.ty(scope, payload, mode);
                quote!(::core::option::Option<#payload>)
            }
            TypeDefKind::Result(result) => {
                let ok = self.optional_ty(scope, result.ok.as_ref(), mode);
                let err = self.optional_ty(scope, result.err.as_ref(), mode);
                quote!(::core::result::Result<#ok, #err>)
            }
            TypeDefKind::Tuple(tuple) => {
                let types = tuple.types.iter().map(|ty| self.ty(scope, ty, mode));
                quote!((#(#types,)*))
            }
            TypeDefKind::Future(payload) => {
                let payload = self.optional_ty(scope, payload.as_ref(), TypeMode::Owned);
                quote!(#c::FutureReader<#payload>)
            }
            TypeDefKind::Stream(payload) => {
                let payload = self.optional_ty(scope, payload.as_ref(), TypeMode::Owned);
                quote!(#c::StreamReader<#payload>)
            }
            TypeDefKind::Handle(handle) => self.handle(scope, handle),
            TypeDefKind::Type(alias) => self.ty(scope, alias, mode),
            TypeDefKind::Map(key, value) => {
                let key = self.ty(scope, key, mode);
                let value = self.ty(scope, value, mode);
                quote!(::std::collections::HashMap<#key, #value>)
            }
            TypeDefKind::FixedLengthList(element, size) => {
                let element = self.ty(scope, element, mode);
                let size = Literal::usize_unsuffixed(*size as usize);
                quote!([#element; #size])
            }
            // A WIT record, variant, enum, flags, or resource type always
            // has a name, and a resolved WIT has no unknown type.
            TypeDefKind::Record(_)
            | TypeDefKind::Variant(_)
            | TypeDefKind::Enum(_)
            | TypeDefKind::Flags(_)
            | TypeDefKind::Resource
            | TypeDefKind::Unknown => quote!(()),
        }
    }

    /// A handle as a Rust type: `Resource<T>` for a resource the host
    /// defines, and `ResourceAny` for one a guest defines, as in
    /// Wasmtime.
    pub fn handle(&self, scope: &[String], handle: &Handle) -> TokenStream {
        let c = &self.c;
        let (Handle::Own(resource) | Handle::Borrow(resource)) = *handle;
        let definition = resolve_type_definition_id(self.resolve, resource);
        let host_defined = match self.resolve.types[definition].owner {
            TypeOwner::Interface(owner) => self.is_imported_interface(owner),
            TypeOwner::World(_) | TypeOwner::None => true,
        };
        if host_defined {
            let path = self.type_path(scope, resource);
            quote!(#c::Resource<#path>)
        } else {
            quote!(#c::ResourceAny)
        }
    }

    /// The WIT documentation `docs` as Rust documentation.
    pub fn rustdoc(docs: &Docs) -> TokenStream {
        let Some(contents) = &docs.contents else {
            return TokenStream::new();
        };
        let lines = contents.lines().map(|line| {
            let line = format!(" {}", line.trim_end());
            quote!(#[doc = #line])
        });
        quote!(#(#lines)*)
    }

    /// The type `id`, named `name` in WIT, defined in the module
    /// `scope`.
    pub fn define_type(&self, scope: &[String], name: &str, id: TypeId) -> TokenStream {
        let ty = &self.resolve.types[id];
        let docs = Self::rustdoc(&ty.docs);
        let rust_name = ident(&to_rust_upper_camel_case(name));
        let definition = match &ty.kind {
            TypeDefKind::Record(record) => self.type_record(scope, id, &rust_name, record),
            TypeDefKind::Variant(variant) => self.type_variant(scope, id, &rust_name, variant),
            TypeDefKind::Enum(enum_) => self.type_enum(id, &rust_name, enum_),
            TypeDefKind::Flags(flags) => return self.type_flags(id, &rust_name, flags, &docs),
            TypeDefKind::Resource => return self.type_resource(scope, id, name, &docs),
            TypeDefKind::Handle(handle) => {
                let handle = self.handle(scope, handle);
                quote!(pub type #rust_name = #handle;)
            }
            TypeDefKind::Type(Type::Id(alias))
                if matches!(
                    self.resolve.types[resolve_type_definition_id(self.resolve, *alias)].kind,
                    TypeDefKind::Resource
                ) =>
            {
                // A resource `use`d from another interface is the same
                // resource: an alias of its type, with no size to check.
                let path = self.type_path(scope, *alias);
                return quote!(#docs pub type #rust_name = #path;);
            }
            TypeDefKind::Option(_)
            | TypeDefKind::Result(_)
            | TypeDefKind::Tuple(_)
            | TypeDefKind::List(_)
            | TypeDefKind::Map(..)
            | TypeDefKind::FixedLengthList(..)
            | TypeDefKind::Future(_)
            | TypeDefKind::Stream(_)
            | TypeDefKind::Type(_) => {
                // An anonymous type with a name is an alias of the
                // anonymous type.
                let aliased = self.type_id_unnamed(scope, id);
                quote!(pub type #rust_name = #aliased;)
            }
            TypeDefKind::Unknown => TokenStream::new(),
        };
        let layout = self.layout_check(id, &rust_name);
        quote! {
            #docs
            #definition
            #layout
        }
    }

    /// The compile-time check that the Rust type `rust_name` of the WIT
    /// type `id` has the WIT type's size and alignment.
    fn layout_check(&self, id: TypeId, rust_name: &syn::Ident) -> TokenStream {
        let c = &self.c;
        let size = Literal::usize_unsuffixed(self.sizes.size(&Type::Id(id)).size_wasm32());
        let align = Literal::usize_unsuffixed(self.sizes.align(&Type::Id(id)).align_wasm32());
        quote! {
            const _: () = {
                assert!(#size == <#rust_name as #c::ComponentType>::SIZE32);
                assert!(#align == <#rust_name as #c::ComponentType>::ALIGN32 as usize);
            };
        }
    }

    /// The type `id` as the anonymous type it names, ignoring its name.
    fn type_id_unnamed(&self, scope: &[String], id: TypeId) -> TokenStream {
        match &self.resolve.types[id].kind {
            TypeDefKind::Type(ty) => self.ty(scope, ty, TypeMode::Owned),
            _ => {
                // Print the kind as though the type had no name.
                let mut anonymous = self.resolve.types[id].clone();
                anonymous.name = None;
                self.anonymous(scope, &anonymous.kind)
            }
        }
    }

    /// An anonymous type of `kind` as a Rust type.
    fn anonymous(&self, scope: &[String], kind: &TypeDefKind) -> TokenStream {
        let c = &self.c;
        match kind {
            TypeDefKind::List(element) => {
                let element = self.ty(scope, element, TypeMode::Owned);
                quote!(::std::vec::Vec<#element>)
            }
            TypeDefKind::Option(payload) => {
                let payload = self.ty(scope, payload, TypeMode::Owned);
                quote!(::core::option::Option<#payload>)
            }
            TypeDefKind::Result(result) => {
                let ok = self.optional_ty(scope, result.ok.as_ref(), TypeMode::Owned);
                let err = self.optional_ty(scope, result.err.as_ref(), TypeMode::Owned);
                quote!(::core::result::Result<#ok, #err>)
            }
            TypeDefKind::Tuple(tuple) => {
                let types = tuple
                    .types
                    .iter()
                    .map(|ty| self.ty(scope, ty, TypeMode::Owned));
                quote!((#(#types,)*))
            }
            TypeDefKind::Future(payload) => {
                let payload = self.optional_ty(scope, payload.as_ref(), TypeMode::Owned);
                quote!(#c::FutureReader<#payload>)
            }
            TypeDefKind::Stream(payload) => {
                let payload = self.optional_ty(scope, payload.as_ref(), TypeMode::Owned);
                quote!(#c::StreamReader<#payload>)
            }
            TypeDefKind::Map(key, value) => {
                let key = self.ty(scope, key, TypeMode::Owned);
                let value = self.ty(scope, value, TypeMode::Owned);
                quote!(::std::collections::HashMap<#key, #value>)
            }
            TypeDefKind::FixedLengthList(element, size) => {
                let element = self.ty(scope, element, TypeMode::Owned);
                let size = Literal::usize_unsuffixed(*size as usize);
                quote!([#element; #size])
            }
            TypeDefKind::Handle(handle) => self.handle(scope, handle),
            TypeDefKind::Type(ty) => self.ty(scope, ty, TypeMode::Owned),
            TypeDefKind::Record(_)
            | TypeDefKind::Variant(_)
            | TypeDefKind::Enum(_)
            | TypeDefKind::Flags(_)
            | TypeDefKind::Resource
            | TypeDefKind::Unknown => quote!(()),
        }
    }

    /// The derives a generated record or variant carries: Wasmtime's
    /// `Clone` and `Copy` where the type allows them, and the
    /// invocation's `additional_derives`.
    fn derives(&self, id: TypeId, extra: &[&str]) -> TokenStream {
        let info = self.infos.get(id);
        let mut derives = BTreeSet::new();
        for path in &self.config.options.additional_derives {
            derives.insert(quote!(#path).to_string());
        }
        if info.is_copy() {
            derives.insert("Clone".to_owned());
            derives.insert("Copy".to_owned());
        } else if info.is_clone() {
            derives.insert("Clone".to_owned());
        }
        for derive in extra {
            derives.insert((*derive).to_owned());
        }
        let derives = derives
            .into_iter()
            .map(|derive| syn::parse_str::<syn::Path>(&derive).expect("a derive is a path"));
        quote!(#[derive(#(#derives),*)])
    }

    /// The attributes of the polyfill's derives, with `wcmp_crate`
    /// where the invocation names one.
    fn component_derives(&self, style: &str) -> TokenStream {
        let c = &self.c;
        let style = ident(style);
        let crate_path = self
            .config
            .options
            .wcmp_crate
            .as_ref()
            .map(|path| quote!(#[component(wcmp_crate = #path)]));
        quote! {
            #[derive(#c::ComponentType, #c::Lift, #c::Lower)]
            #[component(#style)]
            #crate_path
        }
    }

    /// `Display` and `Error` for a type a function returns as the error
    /// of its `result`, as Wasmtime writes them.
    fn error_impls(&self, id: TypeId, name: &syn::Ident) -> TokenStream {
        if !self.infos.get(id).error {
            return TokenStream::new();
        }
        quote! {
            impl ::core::fmt::Display for #name {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    write!(f, "{:?}", self)
                }
            }
            impl ::core::error::Error for #name {}
        }
    }

    /// A WIT record as a Rust struct.
    fn type_record(
        &self,
        scope: &[String],
        id: TypeId,
        name: &syn::Ident,
        record: &Record,
    ) -> TokenStream {
        let component = self.component_derives("record");
        let derives = self.derives(id, &["Debug"]);
        let fields = record.fields.iter().map(|field| {
            let docs = Self::rustdoc(&field.docs);
            let wit = &field.name;
            let field_name = ident(&to_rust_ident(&field.name));
            let ty = self.ty(scope, &field.ty, TypeMode::Owned);
            quote! {
                #docs
                #[component(name = #wit)]
                pub #field_name: #ty,
            }
        });
        let errors = self.error_impls(id, name);
        quote! {
            #component
            #derives
            pub struct #name {
                #(#fields)*
            }
            #errors
        }
    }

    /// A WIT variant as a Rust enum with payloads.
    fn type_variant(
        &self,
        scope: &[String],
        id: TypeId,
        name: &syn::Ident,
        variant: &Variant,
    ) -> TokenStream {
        let component = self.component_derives("variant");
        let derives = self.derives(id, &["Debug"]);
        let cases = variant.cases.iter().map(|case| {
            let docs = Self::rustdoc(&case.docs);
            let wit = &case.name;
            let case_name = format_ident!("{}", case.name.to_upper_camel_case());
            let payload = case.ty.as_ref().map(|ty| {
                let ty = self.ty(scope, ty, TypeMode::Owned);
                quote!((#ty))
            });
            quote! {
                #docs
                #[component(name = #wit)]
                #case_name #payload,
            }
        });
        let errors = self.error_impls(id, name);
        quote! {
            #component
            #derives
            pub enum #name {
                #(#cases)*
            }
            #errors
        }
    }

    /// A WIT enum as a Rust enum of unit cases, with Wasmtime's `repr`.
    fn type_enum(&self, id: TypeId, name: &syn::Ident, enum_: &Enum) -> TokenStream {
        let component = self.component_derives("enum");
        let derives = self.derives(id, &["Clone", "Copy", "Debug", "PartialEq", "Eq"]);
        let repr = match enum_.cases.len().max(1).ilog2() {
            0..=7 => quote!(u8),
            8..=15 => quote!(u16),
            _ => quote!(u32),
        };
        let cases = enum_.cases.iter().map(|case| {
            let docs = Self::rustdoc(&case.docs);
            let wit = &case.name;
            let case_name = format_ident!("{}", case.name.to_upper_camel_case());
            quote! {
                #docs
                #[component(name = #wit)]
                #case_name,
            }
        });
        let errors = self.error_impls(id, name);
        quote! {
            #component
            #derives
            #[repr(#repr)]
            pub enum #name {
                #(#cases)*
            }
            #errors
        }
    }

    /// A WIT flags type, through the polyfill's `flags!`, which takes the
    /// type's docs: rustdoc reads no attribute on a macro call.
    fn type_flags(
        &self,
        id: TypeId,
        name: &syn::Ident,
        flags: &Flags,
        docs: &TokenStream,
    ) -> TokenStream {
        let c = &self.c;
        let crate_path = self
            .config
            .options
            .wcmp_crate
            .as_ref()
            .map(|path| quote!(#[component(wcmp_crate = #path)]));
        let flags = flags.flags.iter().map(|flag| {
            let wit = &flag.name;
            let flag_name = format_ident!("{}", flag.name.to_shouty_snake_case());
            quote!(#[component(name = #wit)] const #flag_name;)
        });
        let layout = self.layout_check(id, name);
        quote! {
            #c::flags!(#docs #crate_path #name { #(#flags)* });
            #layout
        }
    }

    /// A WIT resource: the empty enum `pub enum Name {}` for a resource
    /// the host defines, and `ResourceAny` for one a guest defines.
    fn type_resource(
        &self,
        scope: &[String],
        id: TypeId,
        name: &str,
        docs: &TokenStream,
    ) -> TokenStream {
        let c = &self.c;
        let rust_name = ident(&name.to_upper_camel_case());
        let (host_defined, key) = match self.resolve.types[id].owner {
            TypeOwner::Interface(owner) => {
                (self.is_imported_interface(owner), self.world_key_of(owner))
            }
            TypeOwner::World(_) | TypeOwner::None => (true, None),
        };
        if host_defined {
            // A resource `with` maps is the host's own type, so its
            // handle is `Resource` of that type.
            if let Some(mapped) = self.lookup_replacement(key, Some(name)) {
                let root = Self::path_to_root(scope);
                return quote! {
                    #docs
                    pub use #root #mapped as #rust_name;
                };
            }
            quote! {
                #docs
                pub enum #rust_name {}
            }
        } else {
            quote! {
                #docs
                pub type #rust_name = #c::ResourceAny;
            }
        }
    }
}
