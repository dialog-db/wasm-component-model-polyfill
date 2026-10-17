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
use crate::type_info::TypeInfos;
use crate::type_mode::TypeMode;

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
        }
    }

    /// Write the code.
    pub fn generate(mut self) -> syn::Result<TokenStream> {
        let world = &self.resolve.worlds[self.world];
        let only_interfaces = self.config.options.only_interfaces;
        let mut root = TokenStream::new();
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

        self.config.options.imports.check_all_used("imports")?;

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
                path.push(
                    self.resolve.packages[package]
                        .name
                        .namespace
                        .to_snake_case(),
                );
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
        let base = package.name.to_snake_case();
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
        let mut items = self.interface_types(&path, id);

        let interface = &self.resolve.interfaces[id];
        let functions = interface
            .functions
            .values()
            .filter(|func| func.kind.resource().is_none())
            .collect::<Vec<_>>();
        let host = self.import_trait(&path, Some(key), "Host", &functions)?;
        items.extend(host.tokens);
        let registrations = functions
            .iter()
            .map(|func| self.import_registration(&path, Some(key), func, "Host"))
            .collect::<syn::Result<Vec<_>>>()?;
        let bounds = self.store_data_bounds();
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
            pub fn add_to_linker_instance<T, D>(
                inst: &mut #c::LinkerInstance<'_, T>,
                host_getter: fn(&mut T) -> D::Data<'_>,
            ) -> #c::Result<()>
            where
                D: HostWithStore<T>,
                for<'a> D::Data<'a>: Host,
                T: 'static #bounds,
            {
                // An interface of resources or types alone registers no
                // function.
                let _ = (&inst, host_getter);
                #(#registrations)*
                #c::Result::Ok(())
            }

            /// Register the host functions of this interface on the
            /// linker, under the interface's name.
            pub fn add_to_linker<T, D>(
                linker: &mut #c::Linker<T>,
                host_getter: fn(&mut T) -> D::Data<'_>,
            ) -> #c::Result<()>
            where
                D: HostWithStore<T>,
                for<'a> D::Data<'a>: Host,
                T: 'static #bounds,
            {
                #instance
                add_to_linker_instance::<T, D>(&mut inst, host_getter)
            }
        });
        self.modules.insert(&path, items);
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
        if functions.is_empty() {
            return Ok(TokenStream::new());
        }
        let c = &self.c;
        let world = to_rust_upper_camel_case(&self.resolve.worlds[self.world].name);
        let name = format!("{world}Imports");
        let host = ident(&name);
        let with_store = format_ident!("{name}WithStore");
        let imports = self.import_trait(&[], None, &name, functions)?;
        let registrations = functions
            .iter()
            .map(|func| self.import_registration(&[], None, func, &name))
            .collect::<syn::Result<Vec<_>>>()?;
        let bounds = self.store_data_bounds();
        let tokens = imports.tokens;
        Ok(quote! {
            #tokens

            /// Register the host functions the world imports outside an
            /// interface on the linker's root.
            pub fn add_to_linker_imports<T, D>(
                linker: &mut #c::Linker<T>,
                host_getter: fn(&mut T) -> D::Data<'_>,
            ) -> #c::Result<()>
            where
                D: #with_store<T>,
                for<'a> D::Data<'a>: #host,
                T: 'static #bounds,
            {
                let mut inst = linker.root();
                #(#registrations)*
                #c::Result::Ok(())
            }
        })
    }

    /// Write the module of the exported interface `id`.
    fn export_interface(&mut self, key: &WorldKey, id: InterfaceId) -> syn::Result<()> {
        let path = self.interface_path(key, id, Direction::Export);
        self.interface_names
            .insert(id, (InterfaceName::Path(path.clone()), Direction::Export));
        let items = self.interface_types(&path, id);
        self.modules.insert(&path, items);
        Ok(())
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
        let c = &self.c;
        let definition = match &ty.kind {
            TypeDefKind::Record(record) => self.type_record(scope, id, &rust_name, record),
            TypeDefKind::Variant(variant) => self.type_variant(scope, id, &rust_name, variant),
            TypeDefKind::Enum(enum_) => self.type_enum(id, &rust_name, enum_),
            TypeDefKind::Flags(flags) => self.type_flags(&rust_name, flags),
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
        let size = Literal::usize_unsuffixed(self.sizes.size(&Type::Id(id)).size_wasm32());
        let align = Literal::usize_unsuffixed(self.sizes.align(&Type::Id(id)).align_wasm32());
        quote! {
            #docs
            #definition
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

    /// A WIT flags type, through the polyfill's `flags!`.
    fn type_flags(&self, name: &syn::Ident, flags: &Flags) -> TokenStream {
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
        quote! {
            #c::flags!(#crate_path #name { #(#flags)* });
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
        let _ = (scope, id);
        let c = &self.c;
        let rust_name = ident(&name.to_upper_camel_case());
        let host_defined = match self.resolve.types[id].owner {
            TypeOwner::Interface(owner) => self.is_imported_interface(owner),
            TypeOwner::World(_) | TypeOwner::None => true,
        };
        if host_defined {
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
