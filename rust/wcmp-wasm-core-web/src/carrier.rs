//! Generated modules that carry a `v128` or an `exnref` across a call.

use std::collections::HashMap;

use js_sys::{Array, Function, Object, Reflect, WebAssembly};
use wasm_bindgen::{JsCast, JsValue};
use wasm_encoder::{
    AbstractHeapType, BlockType, CodeSection, EntityType, ExportKind, ExportSection,
    FunctionSection, ImportSection, TypeSection,
};
use wcmp_wasm_core::backend::{RawHandle, StoreId};
use wcmp_wasm_core::{Error, ExnRef, FuncType, HeapType, Result, Val, ValType};

use crate::errors;
use crate::js;
use crate::objects::Objects;
use crate::type_registry::TypeRegistry;
use crate::values::{self, Kind};

/// The generated modules through which the host calls a guest function
/// whose type holds a `v128` or an `exnref`.
///
/// The JavaScript API carries neither value between JavaScript and a
/// guest. So for such a function, the backend generates a carrier: a small
/// module that imports the function and exports one function that the
/// JavaScript API can call. The carrier takes and gives a `v128` as two
/// `i64` halves, and an `exnref` as its index in a table of exceptions that
/// the store owns, or `-1` for null. Inside the carrier, the call of the
/// guest function is a call from WebAssembly to WebAssembly.
///
/// The table of exceptions roots each `exnref` that crosses to the host
/// for the life of the store: the index is the handle, and the store never
/// takes an entry out. The table is itself a generated module's, since the
/// JavaScript API makes no table of `exnref`.
pub struct Carrier {
    id: StoreId,
    exns: Option<WebAssembly::Table>,
    modules: HashMap<FuncType, WebAssembly::Module>,
    functions: HashMap<u64, Function>,
}

/// How a carrier takes or gives one value of the guest function's type.
#[derive(Clone, Copy)]
enum Slot {
    /// A value the JavaScript API carries, as it is.
    Plain(wasm_encoder::ValType),
    /// A `v128`, as its low and its high `i64` halves.
    V128,
    /// A reference to an exception, as its index in the table of
    /// exceptions, or `-1` for null.
    Exn { nullable: bool },
    /// A null reference to an exception, as `-1`.
    NoExn,
}

impl Carrier {
    /// The carriers of the store `id`, none yet.
    pub fn new(id: StoreId) -> Self {
        Self {
            id,
            exns: None,
            modules: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    /// Whether a call of a function of type `ty` needs a carrier.
    pub fn needed(ty: &FuncType) -> bool {
        ty.params()
            .iter()
            .chain(ty.results())
            .any(|ty| matches!(slot(ty), Some(Slot::V128 | Slot::Exn { .. } | Slot::NoExn)))
    }

    /// Calls the guest function `func`, whose handle has index `index` in
    /// `objects` and whose type is `ty`, with `params`, through its carrier,
    /// and writes its results to `results`.
    ///
    /// The engine already checked the number and the kinds of the values.
    pub fn call(
        &mut self,
        objects: &mut Objects,
        types: &TypeRegistry,
        (index, func): (u64, &Function),
        ty: &FuncType,
        params: &[Val],
        results: &mut [Val],
    ) -> Result<()> {
        let carrier = self.function(index, func, ty)?;
        let mut args = Vec::new();
        for (value, ty) in params.iter().zip(ty.params()) {
            match (slot(ty), value) {
                (Some(Slot::V128), Val::V128(bits)) => {
                    args.push(JsValue::from(*bits as u64 as i64));
                    args.push(JsValue::from((*bits >> 64) as u64 as i64));
                }
                (Some(Slot::Exn { .. } | Slot::NoExn), Val::ExnRef(exn)) => {
                    args.push(JsValue::from_f64(self.exn_index(*exn)?));
                }
                (Some(Slot::Exn { .. } | Slot::NoExn), value) if value.is_null() => {
                    args.push(JsValue::from_f64(-1.0));
                }
                _ => args.push(values::to_js(objects, value)?),
            }
        }
        let returned = Reflect::apply(&carrier, &JsValue::UNDEFINED, &args.into_iter().collect())
            .map_err(|error| errors::call(&error))?;
        let width: usize = ty.results().iter().map(|ty| slot_width(slot(ty))).sum();
        let mut returned = match width {
            0 => Vec::new(),
            1 => vec![returned],
            _ => returned
                .dyn_into::<Array>()
                .map_err(|_| values::mismatch("the carrier gave one result".to_string()))?
                .to_vec(),
        }
        .into_iter();
        let mut next = || {
            returned
                .next()
                .ok_or_else(|| values::mismatch("the carrier gave too few results".to_string()))
        };
        for (result, ty) in results.iter_mut().zip(ty.results()) {
            *result = match slot(ty) {
                Some(Slot::V128) => {
                    let low = half(next()?)?;
                    let high = half(next()?)?;
                    Val::V128(u128::from(low) | (u128::from(high) << 64))
                }
                Some(Slot::Exn { .. } | Slot::NoExn) => {
                    let index = next()?.as_f64().unwrap_or(-1.0);
                    Val::ExnRef((index >= 0.0).then(|| ExnRef::from_raw(self.id, index as u64)))
                }
                _ => values::from_js(objects, next()?, Kind::of_type(ty, types))?,
            };
        }
        Ok(())
    }

    /// The index of `exn` in the table of exceptions, or `-1` for null.
    fn exn_index(&mut self, exn: Option<ExnRef>) -> Result<f64> {
        let Some(exn) = exn else {
            return Ok(-1.0);
        };
        let size = js::get(self.exns()?, "length")
            .ok()
            .as_ref()
            .and_then(js::count)
            .unwrap_or(0);
        // The store hands out the index of each exception it holds, so an
        // index past the table is a handle the host forged.
        if exn.index() >= size {
            return Err(Error::WrongStore);
        }
        Ok(exn.index() as f64)
    }

    /// The table of exceptions of the store, made the first time a call
    /// needs it.
    fn exns(&mut self) -> Result<&WebAssembly::Table> {
        if self.exns.is_none() {
            let bytes = wcmp_macros::wasm!(r#"(module (table (export "exns") 0 exnref))"#);
            let exports = instantiate(bytes, &Object::new())?;
            let table = js::get(&exports, "exns")
                .ok()
                .and_then(|table| table.dyn_into::<WebAssembly::Table>().ok())
                .ok_or_else(|| errors::backend("the table of exceptions did not load"))?;
            self.exns = Some(table);
        }
        self.exns
            .as_ref()
            .ok_or_else(|| errors::backend("the table of exceptions did not load"))
    }

    /// The carrier function of the guest function `func`, whose handle has
    /// index `index`, made the first time the host calls it.
    fn function(&mut self, index: u64, func: &Function, ty: &FuncType) -> Result<Function> {
        if let Some(carrier) = self.functions.get(&index) {
            return Ok(carrier.clone());
        }
        let uses_exns = ty
            .params()
            .iter()
            .chain(ty.results())
            .any(|ty| matches!(slot(ty), Some(Slot::Exn { .. } | Slot::NoExn)));
        let imports =
            js::object(&[("callee", func.clone().into())]).map_err(|error| errors::call(&error))?;
        if uses_exns {
            let exns = self.exns()?.clone();
            js::set(&imports, "exns", &exns).map_err(|error| errors::call(&error))?;
        }
        let module = match self.modules.get(ty) {
            Some(module) => module.clone(),
            None => {
                let bytes = generate(ty, uses_exns)?;
                let module = WebAssembly::Module::new(&js_sys::Uint8Array::from(&bytes[..]).into())
                    .map_err(|error| errors::backend(errors::message(&error)))?;
                self.modules.insert(ty.clone(), module.clone());
                module
            }
        };
        let imports_object =
            js::object(&[("", imports.into())]).map_err(|error| errors::call(&error))?;
        let exports = WebAssembly::Instance::new(&module, &imports_object)
            .map_err(|error| errors::backend(errors::message(&error)))?
            .exports();
        let carrier = js::get(&exports, "call")
            .ok()
            .and_then(|carrier| carrier.dyn_into::<Function>().ok())
            .ok_or_else(|| errors::backend("the carrier exports no function"))?;
        self.functions.insert(index, carrier.clone());
        Ok(carrier)
    }
}

/// The exports of the generated module `bytes`, instantiated with
/// `imports`.
///
/// A generated module is small, so it compiles and instantiates
/// synchronously.
fn instantiate(bytes: &[u8], imports: &Object) -> Result<Object> {
    let module = WebAssembly::Module::new(&js_sys::Uint8Array::from(bytes).into())
        .map_err(|error| errors::backend(errors::message(&error)))?;
    Ok(WebAssembly::Instance::new(&module, imports)
        .map_err(|error| errors::backend(errors::message(&error)))?
        .exports())
}

/// The half of a `v128` that the JavaScript API gave as the `BigInt`
/// `value`.
fn half(value: JsValue) -> Result<u64> {
    i64::try_from(value)
        .map(|half| half as u64)
        .map_err(|value| values::mismatch(format!("{value:?} is not a half of a v128")))
}

/// How a carrier takes or gives a value of `ty`, or `None` where no
/// generated module can name `ty`: a reference to a concrete type, whose
/// definition the carrier does not have.
fn slot(ty: &ValType) -> Option<Slot> {
    use wasm_encoder::ValType as Encoded;
    Some(match ty {
        ValType::I32 => Slot::Plain(Encoded::I32),
        ValType::I64 => Slot::Plain(Encoded::I64),
        ValType::F32 => Slot::Plain(Encoded::F32),
        ValType::F64 => Slot::Plain(Encoded::F64),
        ValType::V128 => Slot::V128,
        ValType::Ref(ty) => {
            let abstract_type = match ty.heap {
                HeapType::Exn => {
                    return Some(Slot::Exn {
                        nullable: ty.nullable,
                    });
                }
                HeapType::NoExn => return Some(Slot::NoExn),
                HeapType::Func => AbstractHeapType::Func,
                HeapType::Extern => AbstractHeapType::Extern,
                HeapType::Any => AbstractHeapType::Any,
                HeapType::Eq => AbstractHeapType::Eq,
                HeapType::I31 => AbstractHeapType::I31,
                HeapType::Struct => AbstractHeapType::Struct,
                HeapType::Array => AbstractHeapType::Array,
                HeapType::Cont => AbstractHeapType::Cont,
                HeapType::NoFunc => AbstractHeapType::NoFunc,
                HeapType::NoExtern => AbstractHeapType::NoExtern,
                HeapType::None => AbstractHeapType::None,
                HeapType::NoCont => AbstractHeapType::NoCont,
                HeapType::Concrete(_) => return None,
            };
            Slot::Plain(Encoded::Ref(wasm_encoder::RefType {
                nullable: ty.nullable,
                heap_type: wasm_encoder::HeapType::Abstract {
                    shared: false,
                    ty: abstract_type,
                },
            }))
        }
    })
}

/// The number of values a carrier takes or gives for one slot.
fn slot_width(slot: Option<Slot>) -> usize {
    match slot {
        Some(Slot::V128) => 2,
        _ => 1,
    }
}

/// The type of a value of `ty` inside the carrier.
fn encoded(ty: &ValType) -> Option<wasm_encoder::ValType> {
    Some(match slot(ty)? {
        Slot::Plain(ty) => ty,
        Slot::V128 => wasm_encoder::ValType::V128,
        Slot::Exn { nullable } => wasm_encoder::ValType::Ref(wasm_encoder::RefType {
            nullable,
            heap_type: wasm_encoder::HeapType::Abstract {
                shared: false,
                ty: AbstractHeapType::Exn,
            },
        }),
        Slot::NoExn => wasm_encoder::ValType::Ref(wasm_encoder::RefType {
            nullable: true,
            heap_type: wasm_encoder::HeapType::Abstract {
                shared: false,
                ty: AbstractHeapType::NoExn,
            },
        }),
    })
}

/// The values a carrier takes or gives for a value of `ty`.
fn carried(ty: &ValType) -> Vec<wasm_encoder::ValType> {
    use wasm_encoder::ValType as Encoded;
    match slot(ty) {
        Some(Slot::Plain(ty)) => vec![ty],
        Some(Slot::V128) => vec![Encoded::I64, Encoded::I64],
        Some(Slot::Exn { .. } | Slot::NoExn) | None => vec![Encoded::I32],
    }
}

/// The carrier module of a function of type `ty`:
///
/// ```text
/// (module
///   (import "" "callee" (func $callee (type $ty)))
///   (import "" "exns" (table $exns 0 exnref))       ;; where `ty` has one
///   (func (export "call") (param ...carried) (result ...carried)
///     ;; each parameter from its carried form
///     call $callee
///     ;; each result into a local, then to its carried form
///     ))
/// ```
fn generate(ty: &FuncType, uses_exns: bool) -> Result<Vec<u8>> {
    use wasm_encoder::ValType as Encoded;
    let concrete = || {
        values::mismatch(
            "the browser backend carries a v128 or an exnref only alongside abstract types"
                .to_string(),
        )
    };
    let params = ty
        .params()
        .iter()
        .map(encoded)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(concrete)?;
    let results = ty
        .results()
        .iter()
        .map(encoded)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(concrete)?;
    let carried_params = ty.params().iter().flat_map(carried).collect::<Vec<_>>();
    let carried_results = ty.results().iter().flat_map(carried).collect::<Vec<_>>();

    let mut module = wasm_encoder::Module::new();
    let mut types = TypeSection::new();
    types.ty().function(params, results.clone());
    types.ty().function(carried_params.clone(), carried_results);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("", "callee", EntityType::Function(0));
    if uses_exns {
        imports.import(
            "",
            "exns",
            EntityType::Table(wasm_encoder::TableType {
                element_type: wasm_encoder::RefType::EXNREF,
                table64: false,
                minimum: 0,
                maximum: None,
                shared: false,
            }),
        );
    }
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(1);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("call", ExportKind::Func, 1);
    module.section(&exports);

    let first_local = carried_params.len() as u32;
    let mut body = wasm_encoder::Function::new(results.iter().map(|ty| (1, *ty)));
    let mut code = body.instructions();
    let mut local = 0;
    for ty in ty.params() {
        match slot(ty) {
            Some(Slot::V128) => {
                code.v128_const(0)
                    .local_get(local)
                    .i64x2_replace_lane(0)
                    .local_get(local + 1)
                    .i64x2_replace_lane(1);
                local += 2;
            }
            Some(Slot::Exn { nullable }) => {
                code.local_get(local)
                    .i32_const(-1)
                    .i32_eq()
                    .if_(BlockType::Result(Encoded::EXNREF))
                    .ref_null(wasm_encoder::HeapType::Abstract {
                        shared: false,
                        ty: AbstractHeapType::Exn,
                    })
                    .else_()
                    .local_get(local)
                    .table_get(0)
                    .end();
                if !nullable {
                    code.ref_as_non_null();
                }
                local += 1;
            }
            Some(Slot::NoExn) => {
                code.ref_null(wasm_encoder::HeapType::Abstract {
                    shared: false,
                    ty: AbstractHeapType::NoExn,
                });
                local += 1;
            }
            Some(Slot::Plain(_)) | None => {
                code.local_get(local);
                local += 1;
            }
        }
    }
    code.call(0);
    for index in (0..results.len() as u32).rev() {
        code.local_set(first_local + index);
    }
    for (index, ty) in ty.results().iter().enumerate() {
        let local = first_local + index as u32;
        match slot(ty) {
            Some(Slot::V128) => {
                code.local_get(local)
                    .i64x2_extract_lane(0)
                    .local_get(local)
                    .i64x2_extract_lane(1);
            }
            Some(Slot::Exn { .. } | Slot::NoExn) => {
                code.local_get(local)
                    .ref_is_null()
                    .if_(BlockType::Result(Encoded::I32))
                    .i32_const(-1)
                    .else_()
                    .local_get(local)
                    .i32_const(1)
                    .table_grow(0)
                    .end();
            }
            Some(Slot::Plain(_)) | None => {
                code.local_get(local);
            }
        }
    }
    code.end();
    let mut section = CodeSection::new();
    section.function(&body);
    module.section(&section);
    Ok(module.finish())
}
