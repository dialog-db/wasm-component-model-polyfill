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
    InterfaceType, TypeComponentInstanceIndex, TypeDef, TypeFuncIndex, TypeModuleIndex,
    TypeResourceTable, TypeResourceTableIndex, TypeTupleIndex,
};
use wasmtime_environ::{EngineOrModuleTypeIndex, EntityType};

use super::extern_type::ExternType;
use super::function_type::{FunctionParameter, FunctionType};
use super::instance_type::{InstanceItem, InstanceType};
use super::module_type::ModuleType;
use crate::error::{Error, Result};
use crate::internal::{CoreValueTypeInternal, ErrorInternal};
use crate::module::{CoreExternType, CoreValueType, ModuleExport, ModuleImport};
use crate::types::{
    EnumType, FixedLengthListType, FlagsType, FutureType, ListType, MapType, OptionType,
    PrimitiveType, RecordField, RecordType, ResourceType, ResultType, StreamType, TupleType,
    ValueType, VariantCase, VariantType,
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

    /// The resource one resource table holds, as the component names
    /// it: the label the resource is imported or exported under, and,
    /// for a table a concrete instance keeps, the table's index.
    pub fn resource(&self, index: TypeResourceTableIndex) -> ResourceType {
        let label = self
            .labels
            .get(&index)
            .map(String::as_str)
            .unwrap_or(UNNAMED_RESOURCE);
        match &self.types[index] {
            TypeResourceTable::Concrete { .. } => {
                ResourceType::indexed(label, index.as_u32() as usize)
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
            InterfaceType::Map(index) => {
                let map = &self.types[*index];
                ValueType::Map(MapType::new(
                    self.value_type(&map.key)?,
                    self.value_type(&map.value)?,
                ))
            }
            InterfaceType::FixedLengthList(index) => {
                let fixed = &self.types[*index];
                ValueType::FixedLengthList(FixedLengthListType::new(
                    self.value_type(&fixed.element)?,
                    fixed.size,
                ))
            }
            // Validation has already refused a payload that holds a
            // `borrow`, and `stream<char>`, so the payload projects as
            // any other value type does.
            InterfaceType::Future(index) => {
                let future = &self.types[self.types[*index].ty];
                ValueType::Future(FutureType::new(match &future.payload {
                    Some(ty) => Some(self.value_type(ty)?),
                    None => None,
                }))
            }
            InterfaceType::Stream(index) => {
                let stream = &self.types[self.types[*index].ty];
                ValueType::Stream(StreamType::new(match &stream.payload {
                    Some(ty) => Some(self.value_type(ty)?),
                    None => None,
                }))
            }
            InterfaceType::ErrorContext(_) => ValueType::ErrorContext,
        })
    }

    /// Project one component-level function type.
    ///
    /// An `async` function type is projected with its flag set. The
    /// projection is the same for an import and for an export; the
    /// caller refuses an `async` import, because no host function
    /// the polyfill registers can satisfy one.
    pub fn function(&self, index: TypeFuncIndex) -> Result<FunctionType> {
        let func = &self.types[index];
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
        Ok(FunctionType {
            parameters,
            // The translator records a function's results as the
            // same tuple of none or one type a `canon task.return`
            // declares, so the one projection serves both.
            result: self.result_tuple(func.results)?,
            async_: func.async_,
        })
    }

    /// Project the result tuple a `canon task.return` declares.
    ///
    /// The built-in takes the result values of the current task as
    /// its own parameters, and the translator records them as a
    /// tuple of none or one type. The polyfill admits at most one
    /// result, so a wider tuple is refused. The results of a
    /// function type are recorded as that same tuple, so
    /// [`function`](Self::function) projects its result through
    /// here.
    pub fn result_tuple(&self, index: TypeTupleIndex) -> Result<Option<ValueType>> {
        let results = &self.types[index].types;
        match results.len() {
            0 => Ok(None),
            1 => Ok(Some(self.value_type(&results[0])?)),
            _ => Err(Error::unsupported("functions with more than one result")),
        }
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

    /// Project one core module type: its imports and exports with
    /// their core types.
    pub fn module(&self, index: TypeModuleIndex) -> Result<ModuleType> {
        let module = &self.types[index];
        let mut imports = Vec::with_capacity(module.imports.len());
        for ((namespace, name), entity) in module.imports.iter() {
            imports.push(ModuleImport {
                module: namespace.clone(),
                name: name.clone(),
                ty: self.entity_type(entity)?,
            });
        }
        let mut exports = Vec::with_capacity(module.exports.len());
        for (name, entity) in module.exports.iter() {
            exports.push(ModuleExport {
                name: name.clone(),
                ty: self.entity_type(entity)?,
            });
        }
        Ok(ModuleType { imports, exports })
    }

    /// Project the type of one core entity a module type names.
    fn entity_type(&self, entity: &EntityType) -> Result<CoreExternType> {
        Ok(match entity {
            EntityType::Function(index) => {
                let func = self.core_function(*index)?;
                CoreExternType::Func {
                    params: func
                        .params()
                        .iter()
                        .map(CoreValueType::from_translator)
                        .collect::<Result<Vec<_>>>()?,
                    results: func
                        .results()
                        .iter()
                        .map(CoreValueType::from_translator)
                        .collect::<Result<Vec<_>>>()?,
                }
            }
            EntityType::Global(global) => CoreExternType::Global {
                content: CoreValueType::from_translator(&global.wasm_ty)?,
                mutable: global.mutability,
            },
            EntityType::Memory(memory) => CoreExternType::Memory {
                minimum_pages: memory.limits.min,
                maximum_pages: memory.limits.max,
                memory64: memory.idx_type == wasmtime_environ::IndexType::I64,
                shared: memory.shared,
            },
            EntityType::Table(table) => CoreExternType::Table {
                element: CoreValueType::from_translator(&wasmtime_environ::WasmValType::Ref(
                    table.ref_type,
                ))?,
                minimum: table.limits.min,
                maximum: table.limits.max,
            },
            EntityType::Tag(tag) => {
                let func = self.core_function(tag.signature)?;
                CoreExternType::Tag {
                    params: func
                        .params()
                        .iter()
                        .map(CoreValueType::from_translator)
                        .collect::<Result<Vec<_>>>()?,
                }
            }
        })
    }

    /// The core function type behind a module-level type index.
    fn core_function(
        &self,
        index: EngineOrModuleTypeIndex,
    ) -> Result<&wasmtime_environ::WasmFuncType> {
        match index {
            EngineOrModuleTypeIndex::Module(index) => {
                Ok(self.types.module_types()[index].unwrap_func())
            }
            EngineOrModuleTypeIndex::Engine(_) | EngineOrModuleTypeIndex::RecGroup(_) => Err(
                Error::internal("a core module type names a function type outside the module"),
            ),
        }
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
                TypeResourceTable::Concrete { .. } => ResourceType::indexed(
                    self.labels.get(index).map(String::as_str).unwrap_or(name),
                    index.as_u32() as usize,
                ),
                TypeResourceTable::Abstract(_) => {
                    ResourceType::new(self.labels.get(index).map(String::as_str).unwrap_or(name))
                }
            }),
            TypeDef::Module(index) => ExternType::Module(self.module(*index)?),
            TypeDef::Component(_) => ExternType::Component,
            TypeDef::CoreFunc(_) => {
                return Err(Error::unsupported(
                    "core function imports and exports at the component level",
                ));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use wasmtime_environ::component::{ComponentTypesBuilder, Export as EnvironExport, Translator};
    use wasmtime_environ::wasmparser::Validator;
    use wasmtime_environ::{ScopeVec, Tunables};
    use wcmp_macros::component;

    use super::*;
    use crate::engine_config::EngineConfig;
    use crate::internal::EngineConfigInternal;

    /// A component whose one export takes two parameters and returns
    /// nothing. Its parameters are the two-wide tuple the test needs.
    const TWO_PARAMETERS: &[u8] = component!(
        r#"
        (component
          (core module $m
            (func (export "run") (param i32 i32)))
          (core instance $i (instantiate $m))
          (func (export "run") (param "a" u32) (param "b" u32)
            (canon lift (core func $i "run"))))
        "#
    );

    #[wcmp_macros::test]
    fn it_refuses_a_result_tuple_wider_than_one() {
        // No `canon task.return` declares two results: validation
        // admits at most one, so the refusal is unreachable from a
        // component. The translator interns the parameters of a
        // function as the same kind of tuple it interns its results
        // as, so a two-parameter function supplies a tuple the
        // projection has to refuse.
        let scope = ScopeVec::new();
        let tunables = Tunables::default_u32();
        let mut validator = Validator::new_with_features(EngineConfig::default().wasm_features());
        let mut builder = ComponentTypesBuilder::new(&validator);
        let (translation, _) = Translator::new(&tunables, &mut validator, &mut builder, &scope)
            .translate(TWO_PARAMETERS)
            .expect("the component translates");
        let (types, _) = builder.finish(&translation.component);
        let projector = TypeProjector::new(&types, &translation.component);

        let (_, (export_index, _)) = translation
            .component
            .exports
            .raw_iter()
            .next()
            .expect("the component exports its function");
        let EnvironExport::LiftedFunction { ty, .. } =
            translation.component.export_items[*export_index]
        else {
            panic!("the export is a lifted function");
        };
        let parameters = types[ty].params;

        let err = projector
            .result_tuple(parameters)
            .expect_err("a two-wide tuple is not a result the polyfill admits");

        assert!(
            matches!(&err, Error::Unsupported { feature } if feature
                == "functions with more than one result"),
            "the refusal names the unsupported shape, not {err}"
        );
    }
}
