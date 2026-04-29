//! Implementation of `Component::new`.
//!
//! The parser drives `wasmparser`'s component-model parser over a
//! byte slice, builds a polyfill-side type space as type sections
//! are decoded, and resolves every import and export's type
//! reference against that space so the surface a parsed component
//! exposes is the polyfill's own data shapes.

use wasmparser::{
    ComponentAlias, ComponentDefinedType, ComponentExport as ParserComponentExport,
    ComponentExternalKind, ComponentFuncType, ComponentImport as ParserComponentImport,
    ComponentOuterAliasKind, ComponentType as ParserComponentType, ComponentTypeRef,
    ComponentValType, Encoding, InstanceTypeDeclaration, Parser, Payload, PrimitiveValType,
    TypeBounds,
};

use super::Component;
use super::component_export::ComponentExport;
use super::component_import::ComponentImport;
use super::extern_type::ExternType;
use super::external_name::ExternalName;
use super::function_type::{FunctionParameter, FunctionType};
use super::instance_type::{InstanceItem, InstanceType};
use super::section_inventory::SectionInventory;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::types::{
    EnumType, FlagsType, ListType, OptionType, PrimitiveType, RecordField, RecordType,
    ResourceType, ResultType, TupleType, ValueType, VariantCase, VariantType,
};

/// One slot of a component's type-index space, resolved to the
/// polyfill's own data shape.
#[derive(Clone, Debug)]
enum TypeEntry {
    Defined(ValueType),
    Function(FunctionType),
    Instance(InstanceType),
    Resource(ResourceType),
    /// Placeholder for nested component types and core-type entries
    /// the polyfill does not yet model; kept so type indices line up
    /// for following entries.
    Opaque,
}

impl TypeEntry {
    fn kind_label(&self) -> &'static str {
        match self {
            TypeEntry::Defined(_) => "a defined value type",
            TypeEntry::Function(_) => "a function type",
            TypeEntry::Instance(_) => "an instance type",
            TypeEntry::Resource(_) => "a resource type",
            TypeEntry::Opaque => "an opaque (component or core) type",
        }
    }
}

/// A type space scoped to a single component (or instance type).
#[derive(Clone, Debug, Default)]
struct TypeSpace {
    entries: Vec<TypeEntry>,
}

impl TypeSpace {
    fn push(&mut self, entry: TypeEntry) {
        self.entries.push(entry);
    }

    fn get(&self, index: u32) -> Result<&TypeEntry> {
        self.entries
            .get(index as usize)
            .ok_or(Error::TypeIndexOutOfBounds { index })
    }

    fn value_type(&self, index: u32) -> Result<ValueType> {
        match self.get(index)? {
            TypeEntry::Defined(ty) => Ok(ty.clone()),
            other => Err(Error::WrongTypeKind {
                index,
                expected: "a defined value type",
                actual: other.kind_label(),
            }),
        }
    }

    fn resource(&self, index: u32) -> Result<ResourceType> {
        match self.get(index)? {
            TypeEntry::Resource(rt) => Ok(rt.clone()),
            other => Err(Error::WrongTypeKind {
                index,
                expected: "a resource type",
                actual: other.kind_label(),
            }),
        }
    }
}

/// Parse the bytes into a [`Component`] value.
pub fn parse_component(_engine: &Engine, bytes: &[u8]) -> Result<Component> {
    let mut type_space = TypeSpace::default();
    let mut sections = SectionInventory::default();
    let mut imports: Vec<ComponentImport> = Vec::new();
    let mut exports: Vec<ComponentExport> = Vec::new();

    // parse_all streams nested module / component bodies inline; track
    // depth so we only act on the outer component's payloads.
    let mut depth: usize = 0;
    let parser = Parser::new(0);

    for payload_result in parser.parse_all(bytes) {
        let payload = payload_result.map_err(parse_err)?;

        match payload {
            Payload::Version { encoding, .. } if depth == 0 && encoding != Encoding::Component => {
                return Err(Error::NotAComponent);
            }
            Payload::ModuleSection { .. } => {
                if depth == 0 {
                    sections.core_modules += 1;
                }
                depth += 1;
            }
            Payload::ComponentSection { .. } => {
                if depth == 0 {
                    sections.nested_components += 1;
                }
                depth += 1;
            }
            Payload::End(_) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Payload::ComponentTypeSection(reader) if depth == 0 => {
                sections.component_types += 1;
                for ty in reader {
                    let ty = ty.map_err(parse_err)?;
                    let entry = lower_top_level_type(&type_space, &ty)?;
                    type_space.push(entry);
                }
            }
            Payload::ComponentImportSection(reader) if depth == 0 => {
                sections.component_imports += 1;
                for import in reader {
                    let import = import.map_err(parse_err)?;
                    imports.push(lower_import(&mut type_space, import)?);
                }
            }
            Payload::ComponentExportSection(reader) if depth == 0 => {
                sections.component_exports += 1;
                for export in reader {
                    let export = export.map_err(parse_err)?;
                    exports.push(lower_export(&type_space, export)?);
                }
            }
            Payload::CoreTypeSection(_) if depth == 0 => sections.core_types += 1,
            Payload::InstanceSection(_) if depth == 0 => sections.core_instances += 1,
            Payload::ComponentInstanceSection(_) if depth == 0 => sections.component_instances += 1,
            Payload::ComponentAliasSection(_) if depth == 0 => sections.aliases += 1,
            Payload::ComponentCanonicalSection(_) if depth == 0 => sections.canonicals += 1,
            Payload::ComponentStartSection { .. } if depth == 0 => sections.starts += 1,
            Payload::CustomSection(_) if depth == 0 => sections.custom_sections += 1,
            _ => {}
        }
    }

    Ok(Component {
        imports: imports.into_boxed_slice(),
        exports: exports.into_boxed_slice(),
        sections,
    })
}

fn parse_err(err: wasmparser::BinaryReaderError) -> Error {
    Error::InvalidComponentBinary {
        message: err.message().to_owned(),
        offset: err.offset(),
    }
}

/// Resolve a single component-type-section entry into a [`TypeEntry`].
fn lower_top_level_type(space: &TypeSpace, ty: &ParserComponentType<'_>) -> Result<TypeEntry> {
    match ty {
        ParserComponentType::Defined(defined) => {
            Ok(TypeEntry::Defined(lower_defined(space, defined)?))
        }
        ParserComponentType::Func(func) => Ok(TypeEntry::Function(lower_func(space, func)?)),
        ParserComponentType::Instance(decls) => {
            Ok(TypeEntry::Instance(lower_instance(space, decls)?))
        }
        ParserComponentType::Component(_) => Ok(TypeEntry::Opaque),
        ParserComponentType::Resource { .. } => {
            // Locally-defined resource. Synthesise a label by index;
            // an import that names a resource will assign a clearer
            // label as part of building its own entry.
            let label = format!("resource#{}", space.entries.len());
            Ok(TypeEntry::Resource(ResourceType::new(label)))
        }
    }
}

/// Resolve a [`ComponentDefinedType`] to a [`ValueType`].
fn lower_defined(space: &TypeSpace, defined: &ComponentDefinedType<'_>) -> Result<ValueType> {
    Ok(match defined {
        ComponentDefinedType::Primitive(p) => ValueType::Primitive(lower_primitive(*p)),
        ComponentDefinedType::Record(fields) => ValueType::Record(RecordType::new(
            fields
                .iter()
                .map(|(name, ty)| {
                    Ok::<_, Error>(RecordField::new((*name).to_owned(), lower_val(space, *ty)?))
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        ComponentDefinedType::Variant(cases) => ValueType::Variant(VariantType::new(
            cases
                .iter()
                .map(|case| {
                    let payload = case.ty.map(|t| lower_val(space, t)).transpose()?;
                    Ok::<_, Error>(VariantCase::new(case.name.to_owned(), payload))
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        ComponentDefinedType::List(ty) => ValueType::List(ListType::new(lower_val(space, *ty)?)),
        ComponentDefinedType::Tuple(elems) => ValueType::Tuple(TupleType::new(
            elems
                .iter()
                .map(|t| lower_val(space, *t))
                .collect::<Result<Vec<_>>>()?,
        )),
        ComponentDefinedType::Flags(names) => ValueType::Flags(FlagsType::new(
            names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
        )),
        ComponentDefinedType::Enum(cases) => ValueType::Enum(EnumType::new(
            cases.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
        )),
        ComponentDefinedType::Option(ty) => {
            ValueType::Option(OptionType::new(lower_val(space, *ty)?))
        }
        ComponentDefinedType::Result { ok, err } => ValueType::Result(ResultType::new(
            ok.map(|t| lower_val(space, t)).transpose()?,
            err.map(|t| lower_val(space, t)).transpose()?,
        )),
        ComponentDefinedType::Own(idx) => ValueType::Own(space.resource(*idx)?),
        ComponentDefinedType::Borrow(idx) => ValueType::Borrow(space.resource(*idx)?),
        ComponentDefinedType::Map(_, _) => todo!("`map<K, V>` parsing — post-MVP roadmap"),
        ComponentDefinedType::FixedLengthList(_, _) => {
            todo!("fixed-length `list<T, N>` parsing — post-MVP roadmap")
        }
        ComponentDefinedType::Future(_) => todo!("`future<T>` parsing — async tier"),
        ComponentDefinedType::Stream(_) => todo!("`stream<T>` parsing — async tier"),
    })
}

fn lower_primitive(p: PrimitiveValType) -> PrimitiveType {
    match p {
        PrimitiveValType::Bool => PrimitiveType::Bool,
        PrimitiveValType::S8 => PrimitiveType::S8,
        PrimitiveValType::U8 => PrimitiveType::U8,
        PrimitiveValType::S16 => PrimitiveType::S16,
        PrimitiveValType::U16 => PrimitiveType::U16,
        PrimitiveValType::S32 => PrimitiveType::S32,
        PrimitiveValType::U32 => PrimitiveType::U32,
        PrimitiveValType::S64 => PrimitiveType::S64,
        PrimitiveValType::U64 => PrimitiveType::U64,
        PrimitiveValType::F32 => PrimitiveType::F32,
        PrimitiveValType::F64 => PrimitiveType::F64,
        PrimitiveValType::Char => PrimitiveType::Char,
        PrimitiveValType::String => PrimitiveType::String,
        PrimitiveValType::ErrorContext => todo!("`error-context` primitive — async tier"),
    }
}

fn lower_val(space: &TypeSpace, val: ComponentValType) -> Result<ValueType> {
    match val {
        ComponentValType::Primitive(p) => Ok(ValueType::Primitive(lower_primitive(p))),
        ComponentValType::Type(idx) => space.value_type(idx),
    }
}

fn lower_func(space: &TypeSpace, func: &ComponentFuncType<'_>) -> Result<FunctionType> {
    if func.async_ {
        todo!("async function type parsing — async tier");
    }
    let parameters = func
        .params
        .iter()
        .map(|(name, ty)| {
            Ok(FunctionParameter {
                name: (*name).to_owned(),
                ty: lower_val(space, *ty)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let result = func.result.map(|t| lower_val(space, t)).transpose()?;
    Ok(FunctionType { parameters, result })
}

fn lower_instance(
    outer: &TypeSpace,
    decls: &[InstanceTypeDeclaration<'_>],
) -> Result<InstanceType> {
    let mut local = outer.clone();
    let mut items: Vec<InstanceItem> = Vec::new();

    for decl in decls {
        match decl {
            InstanceTypeDeclaration::Type(ty) => {
                let entry = lower_top_level_type(&local, ty)?;
                local.push(entry);
            }
            InstanceTypeDeclaration::Alias(alias) => match alias {
                ComponentAlias::Outer {
                    kind: ComponentOuterAliasKind::Type,
                    count: 0,
                    index,
                } => {
                    let entry = local.get(*index)?.clone();
                    local.push(entry);
                }
                _ => todo!("non-trivial alias forms inside instance types"),
            },
            InstanceTypeDeclaration::Export { name, ty } => {
                let extern_ty = lower_type_ref(&local, *ty)?;
                items.push(InstanceItem {
                    name: name.0.to_owned(),
                    ty: extern_ty,
                });
            }
            InstanceTypeDeclaration::CoreType(_) => {
                // Core type declarations don't surface in the
                // polyfill's instance-item view; record them in the
                // local space so index alignment is preserved for
                // following items.
                local.push(TypeEntry::Opaque);
            }
        }
    }

    Ok(InstanceType { items })
}

fn lower_type_ref(space: &TypeSpace, tref: ComponentTypeRef) -> Result<ExternType> {
    Ok(match tref {
        ComponentTypeRef::Func(idx) => match space.get(idx)? {
            TypeEntry::Function(f) => ExternType::Function(f.clone()),
            other => {
                return Err(Error::WrongTypeKind {
                    index: idx,
                    expected: "a function type",
                    actual: other.kind_label(),
                });
            }
        },
        ComponentTypeRef::Instance(idx) => match space.get(idx)? {
            TypeEntry::Instance(i) => ExternType::Instance(i.clone()),
            other => {
                return Err(Error::WrongTypeKind {
                    index: idx,
                    expected: "an instance type",
                    actual: other.kind_label(),
                });
            }
        },
        ComponentTypeRef::Module(_) => ExternType::Module,
        ComponentTypeRef::Component(_) => ExternType::Component,
        ComponentTypeRef::Type(bound) => match bound {
            TypeBounds::SubResource => ExternType::Resource(ResourceType::new(format!(
                "resource#{}",
                space.entries.len()
            ))),
            TypeBounds::Eq(idx) => ExternType::ResourceEquals(space.resource(idx)?),
        },
        ComponentTypeRef::Value(val) => ExternType::Value(lower_val(space, val)?),
    })
}

/// The type-space entry that corresponds to a typeref appearing in
/// an import — used to keep the type-space in sync when a component
/// imports a type, function, instance, or resource.
fn type_entry_for_typeref(space: &TypeSpace, tref: ComponentTypeRef) -> Result<TypeEntry> {
    Ok(match tref {
        ComponentTypeRef::Func(idx) => space.get(idx)?.clone(),
        ComponentTypeRef::Instance(idx) => space.get(idx)?.clone(),
        ComponentTypeRef::Component(_) => TypeEntry::Opaque,
        ComponentTypeRef::Module(_) => TypeEntry::Opaque,
        ComponentTypeRef::Type(bound) => match bound {
            TypeBounds::SubResource => TypeEntry::Resource(ResourceType::new(format!(
                "resource#{}",
                space.entries.len()
            ))),
            TypeBounds::Eq(idx) => TypeEntry::Resource(space.resource(idx)?),
        },
        ComponentTypeRef::Value(val) => TypeEntry::Defined(lower_val(space, val)?),
    })
}

fn lower_import(
    space: &mut TypeSpace,
    import: ParserComponentImport<'_>,
) -> Result<ComponentImport> {
    let extern_ty = lower_type_ref(space, import.ty)?;
    // An `(import "x" (type ...))` adds a new type slot the rest of
    // the component can reference; mirror that here so subsequent
    // imports/exports resolve type indices correctly.
    if matches!(import.ty, ComponentTypeRef::Type(_)) {
        let entry = type_entry_for_typeref(space, import.ty)?;
        space.push(entry);
    }
    Ok(ComponentImport {
        name: ExternalName::from_raw(import.name.0),
        ty: extern_ty,
    })
}

fn lower_export(space: &TypeSpace, export: ParserComponentExport<'_>) -> Result<ComponentExport> {
    let ty = match export.ty {
        Some(tref) => lower_type_ref(space, tref)?,
        None => extern_type_for_kind(export.kind),
    };
    Ok(ComponentExport {
        name: ExternalName::from_raw(export.name.0),
        ty,
    })
}

/// Best-effort extern type for an export that does not carry an
/// explicit type ascription. The polyfill does not yet track every
/// kind's index space (functions, instances, modules, …), so this
/// surface reports the kind alone with an empty shape — sufficient
/// for the introspection contract of this slice.
fn extern_type_for_kind(kind: ComponentExternalKind) -> ExternType {
    match kind {
        ComponentExternalKind::Func => ExternType::Function(FunctionType {
            parameters: Vec::new(),
            result: None,
        }),
        ComponentExternalKind::Instance => ExternType::Instance(InstanceType { items: Vec::new() }),
        ComponentExternalKind::Module => ExternType::Module,
        ComponentExternalKind::Component => ExternType::Component,
        ComponentExternalKind::Type => ExternType::Resource(ResourceType::new("resource#?")),
        ComponentExternalKind::Value => {
            todo!("looking up an unascribed value export's type — out of scope for parsing slice")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new().expect("engine constructs")
    }

    #[test]
    fn it_parses_an_empty_component() {
        let bytes = wcmp_macros::component!("(component)");
        let component = parse_component(&engine(), bytes).expect("parses");
        assert!(component.imports.is_empty());
        assert!(component.exports.is_empty());
    }

    #[test]
    fn it_rejects_a_core_module() {
        let bytes = wcmp_macros::wasm!("(module)");
        let err = parse_component(&engine(), bytes).expect_err("rejects core module");
        assert!(matches!(err, Error::NotAComponent), "got {err:?}");
    }

    #[test]
    fn it_rejects_garbage_bytes() {
        let err = parse_component(&engine(), &[0, 1, 2, 3]).expect_err("rejects garbage");
        assert!(
            matches!(err, Error::InvalidComponentBinary { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn it_rejects_a_truncated_preamble() {
        let err = parse_component(&engine(), b"\0asm\x0d\0").expect_err("rejects truncation");
        assert!(
            matches!(err, Error::InvalidComponentBinary { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn it_lowers_a_function_signature_into_polyfill_types() {
        let bytes = wcmp_macros::component!(
            r#"
            (component
              (type (func (param "x" s32) (param "y" u64) (result f32)))
              (import "do-it" (func (type 0))))
        "#
        );

        let component = parse_component(&engine(), bytes).expect("parses");
        assert_eq!(component.imports.len(), 1);
        let import = &component.imports[0];
        assert_eq!(
            import.name,
            ExternalName::Plain("do-it".to_owned()),
            "import name should be a plain label",
        );
        let func = match &import.ty {
            ExternType::Function(f) => f,
            other => panic!("expected a function import, got {other:?}"),
        };
        assert_eq!(func.parameters.len(), 2);
        assert_eq!(func.parameters[0].name, "x");
        assert_eq!(
            func.parameters[0].ty,
            ValueType::Primitive(PrimitiveType::S32)
        );
        assert_eq!(func.parameters[1].name, "y");
        assert_eq!(
            func.parameters[1].ty,
            ValueType::Primitive(PrimitiveType::U64)
        );
        assert_eq!(func.result, Some(ValueType::Primitive(PrimitiveType::F32)));
    }

    #[test]
    fn it_recognises_an_interface_import_name() {
        let bytes = wcmp_macros::component!(
            r#"
            (component
              (type (instance))
              (import "wasi:cli/run@0.2.0" (instance (type 0))))
        "#
        );

        let component = parse_component(&engine(), bytes).expect("parses");
        let import = &component.imports[0];
        match &import.name {
            ExternalName::Interface(id) => {
                assert_eq!(id.package().namespace(), "wasi");
                assert_eq!(id.package().name(), "cli");
                assert_eq!(id.name(), "run");
            }
            other => panic!("expected an interface import name, got {other:?}"),
        }
    }

    #[test]
    fn it_falls_back_to_plain_for_a_kebab_case_import() {
        let bytes = wcmp_macros::component!(
            r#"
            (component
              (type (func))
              (import "hello-world" (func (type 0))))
        "#
        );

        let component = parse_component(&engine(), bytes).expect("parses");
        assert_eq!(
            component.imports[0].name,
            ExternalName::Plain("hello-world".to_owned())
        );
    }

    #[test]
    fn it_counts_top_level_sections_independently_of_nested_module_bodies() {
        let bytes = wcmp_macros::component!(
            r#"
            (component
              (core module $m
                (func (export "f") (result i32) i32.const 42))
              (core instance $i (instantiate $m))
              (func (export "f") (canon lift (core func $i "f"))))
        "#
        );

        let component = parse_component(&engine(), bytes).expect("parses");
        assert_eq!(component.sections.core_modules, 1);
        assert_eq!(component.sections.core_instances, 1);
        assert_eq!(component.sections.canonicals, 1);
        assert_eq!(component.sections.component_exports, 1);
        // The nested core module declares a `(func ...)` of its own,
        // but its sections live inside the module body. The outer
        // component does not pick them up as `core_types` or any
        // other top-level kind — that count belongs to the *outer*
        // component's core-type section.
        assert_eq!(component.sections.core_types, 0);
    }

    #[test]
    fn it_lowers_a_record_value_type() {
        let bytes = wcmp_macros::component!(
            r#"
            (component
              (type $point (record (field "x" s32) (field "y" s32)))
              (type (func (param "p" $point)))
              (import "use-point" (func (type 1))))
        "#
        );

        let component = parse_component(&engine(), bytes).expect("parses");
        let func = match &component.imports[0].ty {
            ExternType::Function(f) => f,
            other => panic!("expected a function, got {other:?}"),
        };
        let record = match &func.parameters[0].ty {
            ValueType::Record(r) => r,
            other => panic!("expected a record, got {other:?}"),
        };
        assert_eq!(record.fields().len(), 2);
        assert_eq!(record.fields()[0].name(), "x");
        assert_eq!(record.fields()[1].name(), "y");
    }
}
