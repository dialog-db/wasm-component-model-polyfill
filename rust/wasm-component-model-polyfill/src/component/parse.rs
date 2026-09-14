//! Implementation of `Component::new`.
//!
//! Parsing is delegated to `wit_component::decode`, which validates
//! the binary and returns a `wit_parser::Resolve` together with the
//! id of the component's `world`. The polyfill walks that high-level
//! WIT view and translates each import and export into the
//! polyfill's own data shapes so the public surface stays
//! runtime-agnostic and never leaks an upstream type.
//!
//! `wit_component::decode` is pure Rust and works on every target
//! the polyfill builds for, so the same lowering drives both native
//! and `wasm32-unknown-unknown` parsing.

use wit_component::DecodedWasm;
use wit_parser::{
    Function, Handle, Interface, InterfaceId, PackageId, PackageName as WitPackageName, Resolve,
    Type as WitType, TypeDefKind, TypeId, World, WorldItem, WorldKey,
};

use super::Component;
use super::component_export::ComponentExport;
use super::component_import::ComponentImport;
use super::extern_type::ExternType;
use super::external_name::ExternalName;
use super::function_type::{FunctionParameter, FunctionType};
use super::instance_type::{InstanceItem, InstanceType};
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::identifier::{InterfaceIdentifier, PackageName};
use crate::types::{
    EnumType, FlagsType, ListType, OptionType, PrimitiveType, RecordField, RecordType,
    ResourceType, ResultType, TupleType, ValueType, VariantCase, VariantType,
};

/// The 8-byte preamble of a Wasm core module: `\0asm` followed by
/// the core-module version word.
const CORE_MODULE_PREAMBLE: [u8; 8] = [b'\0', b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];

/// Parse the bytes into a [`Component`] value.
pub fn parse_component(_engine: &Engine, bytes: &[u8]) -> Result<Component> {
    // wit_component::decode reports core-module bytes via a generic
    // decoding error rather than a structured "this is a module"
    // signal. Sniff the preamble first so the polyfill can surface
    // the dedicated `NotAComponent` variant.
    if bytes.len() >= CORE_MODULE_PREAMBLE.len()
        && bytes[..CORE_MODULE_PREAMBLE.len()] == CORE_MODULE_PREAMBLE
    {
        return Err(Error::NotAComponent);
    }

    let decoded = wit_component::decode(bytes).map_err(invalid_binary)?;

    let (resolve, world_id) = match decoded {
        DecodedWasm::Component(resolve, world_id) => (resolve, world_id),
        DecodedWasm::WitPackage(_, _) => return Err(Error::NotAComponent),
    };

    let world: &World = &resolve.worlds[world_id];

    let imports: Vec<ComponentImport> = world
        .imports
        .iter()
        .map(|(key, item)| {
            let (name, ty) = lower_world_item(&resolve, key, item);
            ComponentImport { name, ty }
        })
        .collect();

    let exports: Vec<ComponentExport> = world
        .exports
        .iter()
        .map(|(key, item)| {
            let (name, ty) = lower_world_item(&resolve, key, item);
            ComponentExport { name, ty }
        })
        .collect();

    Ok(Component {
        imports: imports.into_boxed_slice(),
        exports: exports.into_boxed_slice(),
        bytes: bytes.to_vec().into_boxed_slice(),
    })
}

fn invalid_binary(err: anyhow::Error) -> Error {
    Error::InvalidComponentBinary {
        message: format!("{err}"),
        offset: 0,
    }
}

/// Translate a single (`WorldKey`, `WorldItem`) entry into the name
/// and extern type the polyfill's [`ComponentImport`] /
/// [`ComponentExport`] expect.
fn lower_world_item(
    resolve: &Resolve,
    key: &WorldKey,
    item: &WorldItem,
) -> (ExternalName, ExternType) {
    match item {
        WorldItem::Function(func) => (
            external_name_for_key(resolve, key, Some(&func.name)),
            ExternType::Function(lower_function(resolve, func)),
        ),
        WorldItem::Interface { id, .. } => (
            external_name_for_key(resolve, key, None),
            ExternType::Instance(lower_interface(resolve, *id)),
        ),
        WorldItem::Type { id: type_id, .. } => {
            let type_def = &resolve.types[*type_id];
            let name = external_name_for_key(resolve, key, type_def.name.as_deref());
            let ty = match &type_def.kind {
                TypeDefKind::Resource => ExternType::Resource(ResourceType::new(
                    type_def
                        .name
                        .clone()
                        .unwrap_or_else(|| "resource".to_owned()),
                )),
                _ => ExternType::Value(lower_type_id(resolve, *type_id)),
            };
            (name, ty)
        }
    }
}

/// Build an [`ExternalName`] from a [`WorldKey`].
///
/// `Name` keys are kebab-case strings the binary already declared
/// verbatim — they round-trip through [`ExternalName::from_raw`] so
/// any that happen to look like an interface identifier still parse
/// as one. `Interface` keys carry no textual form; the polyfill
/// reconstructs an interface identifier from the interface's
/// package and own name. `fallback` is consulted only when an
/// interface key references an interface whose own name is missing
/// (an inline interface), in which case the import's underlying
/// item name (a function name, a type name) is the best textual
/// witness available.
fn external_name_for_key(
    resolve: &Resolve,
    key: &WorldKey,
    fallback: Option<&str>,
) -> ExternalName {
    match key {
        WorldKey::Name(name) => ExternalName::from_raw(name),
        WorldKey::Interface(id) => match interface_identifier(resolve, *id) {
            Some(id) => ExternalName::Interface(id),
            None => ExternalName::Plain(
                fallback
                    .map(str::to_owned)
                    .unwrap_or_else(|| "<anonymous interface>".to_owned()),
            ),
        },
    }
}

/// Build the polyfill's [`InterfaceIdentifier`] from a wit-parser
/// interface, when the interface carries enough information to
/// produce one (a name and a package).
fn interface_identifier(resolve: &Resolve, id: InterfaceId) -> Option<InterfaceIdentifier> {
    let iface: &Interface = &resolve.interfaces[id];
    let iface_name = iface.name.clone()?;
    let pkg_id = iface.package?;
    Some(InterfaceIdentifier::new(
        package_name(resolve, pkg_id),
        iface_name,
    ))
}

fn package_name(resolve: &Resolve, id: PackageId) -> PackageName {
    let WitPackageName {
        namespace,
        name,
        version,
    } = &resolve.packages[id].name;
    PackageName::new(namespace.clone(), name.clone(), version.clone())
}

fn lower_interface(resolve: &Resolve, id: InterfaceId) -> InstanceType {
    let iface: &Interface = &resolve.interfaces[id];
    let mut items: Vec<InstanceItem> = Vec::new();

    for (name, type_id) in &iface.types {
        let type_def = &resolve.types[*type_id];
        let ty = match &type_def.kind {
            TypeDefKind::Resource => ExternType::Resource(ResourceType::new(name.clone())),
            _ => ExternType::Value(lower_type_id(resolve, *type_id)),
        };
        items.push(InstanceItem {
            name: name.clone(),
            ty,
        });
    }

    for (name, function) in &iface.functions {
        items.push(InstanceItem {
            name: name.clone(),
            ty: ExternType::Function(lower_function(resolve, function)),
        });
    }

    InstanceType { items }
}

fn lower_function(resolve: &Resolve, func: &Function) -> FunctionType {
    let parameters = func
        .params
        .iter()
        .map(|param| FunctionParameter {
            name: param.name.clone(),
            ty: lower_type(resolve, param.ty),
        })
        .collect();
    let result = func.result.map(|ty| lower_type(resolve, ty));
    FunctionType { parameters, result }
}

fn lower_type(resolve: &Resolve, ty: WitType) -> ValueType {
    match ty {
        WitType::Bool => ValueType::Primitive(PrimitiveType::Bool),
        WitType::S8 => ValueType::Primitive(PrimitiveType::S8),
        WitType::U8 => ValueType::Primitive(PrimitiveType::U8),
        WitType::S16 => ValueType::Primitive(PrimitiveType::S16),
        WitType::U16 => ValueType::Primitive(PrimitiveType::U16),
        WitType::S32 => ValueType::Primitive(PrimitiveType::S32),
        WitType::U32 => ValueType::Primitive(PrimitiveType::U32),
        WitType::S64 => ValueType::Primitive(PrimitiveType::S64),
        WitType::U64 => ValueType::Primitive(PrimitiveType::U64),
        WitType::F32 => ValueType::Primitive(PrimitiveType::F32),
        WitType::F64 => ValueType::Primitive(PrimitiveType::F64),
        WitType::Char => ValueType::Primitive(PrimitiveType::Char),
        WitType::String => ValueType::Primitive(PrimitiveType::String),
        WitType::ErrorContext => todo!("`error-context` type — async tier"),
        WitType::Id(id) => lower_type_id(resolve, id),
    }
}

fn lower_type_id(resolve: &Resolve, id: TypeId) -> ValueType {
    let type_def = &resolve.types[id];
    match &type_def.kind {
        TypeDefKind::Type(inner) => lower_type(resolve, *inner),
        TypeDefKind::Record(rec) => {
            ValueType::Record(RecordType::new(rec.fields.iter().map(|field| {
                RecordField::new(field.name.clone(), lower_type(resolve, field.ty))
            })))
        }
        TypeDefKind::Variant(var) => {
            ValueType::Variant(VariantType::new(var.cases.iter().map(|case| {
                VariantCase::new(case.name.clone(), case.ty.map(|ty| lower_type(resolve, ty)))
            })))
        }
        TypeDefKind::List(inner) => ValueType::List(ListType::new(lower_type(resolve, *inner))),
        TypeDefKind::Tuple(tuple) => ValueType::Tuple(TupleType::new(
            tuple.types.iter().map(|ty| lower_type(resolve, *ty)),
        )),
        TypeDefKind::Flags(flags) => {
            ValueType::Flags(FlagsType::new(flags.flags.iter().map(|f| f.name.clone())))
        }
        TypeDefKind::Enum(en) => {
            ValueType::Enum(EnumType::new(en.cases.iter().map(|c| c.name.clone())))
        }
        TypeDefKind::Option(inner) => {
            ValueType::Option(OptionType::new(lower_type(resolve, *inner)))
        }
        TypeDefKind::Result(res) => ValueType::Result(ResultType::new(
            res.ok.map(|ty| lower_type(resolve, ty)),
            res.err.map(|ty| lower_type(resolve, ty)),
        )),
        TypeDefKind::Handle(Handle::Own(target)) => {
            ValueType::Own(resource_handle_target(resolve, *target))
        }
        TypeDefKind::Handle(Handle::Borrow(target)) => {
            ValueType::Borrow(resource_handle_target(resolve, *target))
        }
        TypeDefKind::Resource => {
            // A resource type appearing in value-type position is
            // unexpected — handles, not resources, are the value
            // form. If we encounter one, it's almost certainly
            // because a `(type ...)` import named the resource
            // directly; surface it as an `own<T>` against the
            // resource's name so the introspection surface still
            // has a value to show.
            ValueType::Own(ResourceType::new(
                type_def
                    .name
                    .clone()
                    .unwrap_or_else(|| "resource".to_owned()),
            ))
        }
        TypeDefKind::Future(_) => todo!("`future<T>` type — async tier"),
        TypeDefKind::Stream(_) => todo!("`stream<T>` type — async tier"),
        TypeDefKind::Unknown => todo!("`unknown` type — non-WIT-conformant import"),
        TypeDefKind::Map(_, _) => todo!("`map<K, V>` lowering — post-MVP roadmap"),
        TypeDefKind::FixedLengthList(_, _) => {
            todo!("fixed-length `list<T, N>` lowering — post-MVP roadmap")
        }
    }
}

fn resource_handle_target(resolve: &Resolve, target: TypeId) -> ResourceType {
    let type_def = &resolve.types[target];
    let label = type_def
        .name
        .clone()
        .unwrap_or_else(|| "resource".to_owned());
    ResourceType::new(label)
}
