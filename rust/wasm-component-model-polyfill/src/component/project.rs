//! Projection of the translator's type information onto the
//! polyfill's own data shapes.
//!
//! The translator ([`wasmtime_environ::component::Translator`])
//! produces a [`ComponentTypes`] table that fully describes every
//! import, export, function, and value type a component declares.
//! [`TypeProjector`] walks that table and produces the polyfill's
//! [`ValueType`], [`FunctionType`], [`InstanceType`], and
//! [`ExternType`] shapes, so that the introspection surface of a
//! [`Component`] and the signatures the executor lifts and lowers
//! against come from one source.
//!
//! Resource types carry a label in the polyfill's shapes. The label
//! is the name under which the resource is imported or exported; the
//! projector collects those names up front so that an `own<T>` or
//! `borrow<T>` deep inside a function type can name its resource.
//!
//! [`Component`]: super::Component

use std::collections::HashMap;

use wasmtime_environ::component::{
    Component as EnvironComponent, ComponentExtern, ComponentTypes, Export as EnvironExport,
    InterfaceType, TypeComponentInstanceIndex, TypeDef, TypeFuncIndex, TypeResourceTable,
    TypeResourceTableIndex,
};

use super::extern_type::ExternType;
use super::function_type::{FunctionParameter, FunctionType};
use super::instance_type::{InstanceItem, InstanceType};
use crate::error::{Error, Result};
use crate::types::{
    EnumType, FlagsType, ListType, OptionType, PrimitiveType, RecordField, RecordType,
    ResourceType, ResultType, TupleType, ValueType, VariantCase, VariantType,
};

/// The label a resource receives when no import or export names it.
const UNNAMED_RESOURCE: &str = "resource";

/// Projects translator types onto polyfill shapes.
pub struct TypeProjector<'a> {
    types: &'a ComponentTypes,
    labels: HashMap<TypeResourceTableIndex, String>,
}

impl<'a> TypeProjector<'a> {
    /// Build a projector for `component`, collecting the label of
    /// every resource the component imports or exports.
    pub fn new(types: &'a ComponentTypes, component: &EnvironComponent) -> Self {
        let mut projector = Self {
            types,
            labels: HashMap::new(),
        };
        for (_, (name, extern_)) in component.import_types.iter() {
            projector.collect_labels(name, &extern_.ty);
        }
        for (name, (export_index, _)) in component.exports.raw_iter() {
            match &component.export_items[*export_index] {
                EnvironExport::Type(def) => projector.collect_labels(name, def),
                EnvironExport::Instance { ty, .. } => projector.collect_instance_labels(*ty),
                EnvironExport::LiftedFunction { .. }
                | EnvironExport::ModuleStatic { .. }
                | EnvironExport::ModuleImport { .. } => {}
            }
        }
        projector
    }

    fn collect_labels(&mut self, name: &str, def: &TypeDef) {
        match def {
            TypeDef::Resource(index) => {
                self.labels.entry(*index).or_insert_with(|| name.to_owned());
            }
            TypeDef::ComponentInstance(index) => self.collect_instance_labels(*index),
            _ => {}
        }
    }

    fn collect_instance_labels(&mut self, index: TypeComponentInstanceIndex) {
        let exports: Vec<(String, TypeDef)> = self.types[index]
            .exports
            .iter()
            .map(|(name, extern_)| (name.clone(), extern_.ty))
            .collect();
        for (name, def) in exports {
            self.collect_labels(&name, &def);
        }
    }

    fn resource(&self, index: TypeResourceTableIndex) -> ResourceType {
        let label = self
            .labels
            .get(&index)
            .map(String::as_str)
            .unwrap_or(UNNAMED_RESOURCE);
        match &self.types[index] {
            TypeResourceTable::Concrete { ty, .. } => {
                ResourceType::indexed(label, ty.as_u32() as usize)
            }
            TypeResourceTable::Abstract(_) => ResourceType::new(label),
        }
    }

    /// Project one value type.
    pub fn value_type(&self, ty: &InterfaceType) -> Result<ValueType> {
        Ok(match ty {
            InterfaceType::Bool => ValueType::Primitive(PrimitiveType::Bool),
            InterfaceType::S8 => ValueType::Primitive(PrimitiveType::S8),
            InterfaceType::U8 => ValueType::Primitive(PrimitiveType::U8),
            InterfaceType::S16 => ValueType::Primitive(PrimitiveType::S16),
            InterfaceType::U16 => ValueType::Primitive(PrimitiveType::U16),
            InterfaceType::S32 => ValueType::Primitive(PrimitiveType::S32),
            InterfaceType::U32 => ValueType::Primitive(PrimitiveType::U32),
            InterfaceType::S64 => ValueType::Primitive(PrimitiveType::S64),
            InterfaceType::U64 => ValueType::Primitive(PrimitiveType::U64),
            InterfaceType::Float32 => ValueType::Primitive(PrimitiveType::F32),
            InterfaceType::Float64 => ValueType::Primitive(PrimitiveType::F64),
            InterfaceType::Char => ValueType::Primitive(PrimitiveType::Char),
            InterfaceType::String => ValueType::Primitive(PrimitiveType::String),
            InterfaceType::Record(index) => {
                let record = &self.types[*index];
                let mut fields = Vec::with_capacity(record.fields.len());
                for field in record.fields.iter() {
                    fields.push(RecordField::new(
                        field.name.clone(),
                        self.value_type(&field.ty)?,
                    ));
                }
                ValueType::Record(RecordType::new(fields))
            }
            InterfaceType::Variant(index) => {
                let variant = &self.types[*index];
                let mut cases = Vec::with_capacity(variant.cases.len());
                for (name, payload) in variant.cases.iter() {
                    let payload = match payload {
                        Some(ty) => Some(self.value_type(ty)?),
                        None => None,
                    };
                    cases.push(VariantCase::new(name.clone(), payload));
                }
                ValueType::Variant(VariantType::new(cases))
            }
            InterfaceType::List(index) => {
                ValueType::List(ListType::new(self.value_type(&self.types[*index].element)?))
            }
            InterfaceType::Tuple(index) => {
                let tuple = &self.types[*index];
                let mut elements = Vec::with_capacity(tuple.types.len());
                for ty in tuple.types.iter() {
                    elements.push(self.value_type(ty)?);
                }
                ValueType::Tuple(TupleType::new(elements))
            }
            InterfaceType::Flags(index) => {
                ValueType::Flags(FlagsType::new(self.types[*index].names.iter().cloned()))
            }
            InterfaceType::Enum(index) => {
                ValueType::Enum(EnumType::new(self.types[*index].names.iter().cloned()))
            }
            InterfaceType::Option(index) => {
                ValueType::Option(OptionType::new(self.value_type(&self.types[*index].ty)?))
            }
            InterfaceType::Result(index) => {
                let result = &self.types[*index];
                let ok = match &result.ok {
                    Some(ty) => Some(self.value_type(ty)?),
                    None => None,
                };
                let err = match &result.err {
                    Some(ty) => Some(self.value_type(ty)?),
                    None => None,
                };
                ValueType::Result(ResultType::new(ok, err))
            }
            InterfaceType::Own(index) => ValueType::Own(self.resource(*index)),
            InterfaceType::Borrow(index) => ValueType::Borrow(self.resource(*index)),
            InterfaceType::Map(_) => return Err(Error::unsupported("`map<K, V>` values")),
            InterfaceType::FixedLengthList(_) => {
                return Err(Error::unsupported("fixed-length `list<T, N>` values"));
            }
            InterfaceType::Future(_) => return Err(Error::unsupported("`future<T>` values")),
            InterfaceType::Stream(_) => return Err(Error::unsupported("`stream<T>` values")),
            InterfaceType::ErrorContext(_) => {
                return Err(Error::unsupported("`error-context` values"));
            }
        })
    }

    /// Project one component-level function type.
    pub fn function(&self, index: TypeFuncIndex) -> Result<FunctionType> {
        let func = &self.types[index];
        if func.async_ {
            return Err(Error::unsupported("asynchronous functions"));
        }
        let params = &self.types[func.params].types;
        let mut parameters = Vec::with_capacity(params.len());
        for (i, ty) in params.iter().enumerate() {
            let name = func
                .param_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("arg{i}"));
            parameters.push(FunctionParameter {
                name,
                ty: self.value_type(ty)?,
            });
        }
        let results = &self.types[func.results].types;
        let result = match results.len() {
            0 => None,
            1 => Some(self.value_type(&results[0])?),
            _ => return Err(Error::unsupported("functions with more than one result")),
        };
        Ok(FunctionType { parameters, result })
    }

    /// Project one instance type into the polyfill's typed bag of
    /// items, in declaration order.
    pub fn instance(&self, index: TypeComponentInstanceIndex) -> Result<InstanceType> {
        let mut items = Vec::new();
        for (name, extern_) in self.types[index].exports.iter() {
            items.push(InstanceItem {
                name: name.clone(),
                ty: self.extern_type(name, extern_)?,
            });
        }
        Ok(InstanceType { items })
    }

    /// Project the type of one import or export.
    pub fn extern_type(&self, name: &str, extern_: &ComponentExtern) -> Result<ExternType> {
        self.type_def(name, &extern_.ty)
    }

    /// Project one [`TypeDef`]. `name` labels a resource type that
    /// has no other name.
    pub fn type_def(&self, name: &str, def: &TypeDef) -> Result<ExternType> {
        Ok(match def {
            TypeDef::ComponentFunc(index) => ExternType::Function(self.function(*index)?),
            TypeDef::ComponentInstance(index) => ExternType::Instance(self.instance(*index)?),
            TypeDef::Interface(ty) => ExternType::Value(self.value_type(ty)?),
            TypeDef::Resource(index) => ExternType::Resource(match &self.types[*index] {
                TypeResourceTable::Concrete { ty, .. } => ResourceType::indexed(
                    self.labels.get(index).map(String::as_str).unwrap_or(name),
                    ty.as_u32() as usize,
                ),
                TypeResourceTable::Abstract(_) => {
                    ResourceType::new(self.labels.get(index).map(String::as_str).unwrap_or(name))
                }
            }),
            TypeDef::Module(_) => ExternType::Module,
            TypeDef::Component(_) => ExternType::Component,
            TypeDef::CoreFunc(_) => {
                return Err(Error::unsupported(
                    "core function imports and exports at the component level",
                ));
            }
        })
    }
}
