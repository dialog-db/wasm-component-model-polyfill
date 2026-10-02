// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Generated modules that carry a float, a `v128`, or an `exnref` across a
//! call.

use std::collections::HashMap;

use js_sys::{Array, Function, Object, WebAssembly};
use wasm_bindgen::closure::Closure;
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
/// whose type holds a float, a `v128`, or an `exnref`.
///
/// The JavaScript API carries neither a `v128` nor an `exnref` between
/// JavaScript and a guest, and it carries a float as a `Number`, which can
/// change the bits of a NaN. So for such a function, the backend generates
/// a carrier: a small module that imports the function and exports one
/// function that the JavaScript API can call. The carrier takes and gives
/// an `f32` as the `i32` of its bits, an `f64` as the `i64` of its bits, a
/// `v128` as two `i64` halves, and an `exnref` as its index in a table of
/// exceptions that the store owns, or `-1` for null. Inside the carrier,
/// the call of the guest function is a call from WebAssembly to
/// WebAssembly.
///
/// The table of exceptions roots each `exnref` that crosses to the host
/// for the life of the store: the index is the handle, and the store never
/// takes an entry out. The table is itself a generated module's, since the
/// JavaScript API makes no table of `exnref`.
pub struct Carrier {
    id: StoreId,
    exns: Option<WebAssembly::Table>,
    modules: HashMap<FuncType, WebAssembly::Module>,
    /// The carrier function of each guest function by the index of its
    /// handle, or `None` for a function whose floats cross as a `Number`:
    /// see [`Carrier::arguments`].
    functions: HashMap<u64, Option<Function>>,
    catcher: Option<Catcher>,
}

/// The generated module that roots an exception which reached the host,
/// and the JavaScript function that throws the exception back into it.
struct Catcher {
    root: Function,
    _rethrow: Closure<dyn Fn(JsValue) -> core::result::Result<(), JsValue>>,
}

/// How a carrier takes or gives one value of the guest function's type.
#[derive(Clone, Copy)]
enum Slot {
    /// A value the JavaScript API carries, as it is.
    Plain(wasm_encoder::ValType),
    /// An `f32`, as the `i32` of its bits.
    F32,
    /// An `f64`, as the `i64` of its bits.
    F64,
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
            catcher: None,
        }
    }

    /// Roots the exception `exception`, a `WebAssembly.Exception` that no
    /// guest caught, in the table of exceptions, and returns its handle.
    ///
    /// The JavaScript API gives the exception to the host as an object, and
    /// has no way to put it in a table of `exnref`. A guest does. So the
    /// backend throws the object back into a generated module, which
    /// catches it as an `exnref` and adds it to the table:
    ///
    /// ```text
    /// (func (export "root") (param externref) (result i32)
    ///   block $caught (result exnref)
    ///     try_table (catch_all_ref $caught)
    ///       local.get 0
    ///       call $rethrow                 ;; JavaScript: throws the object
    ///     end
    ///     i32.const -1
    ///     return
    ///   end
    ///   i32.const 1
    ///   table.grow $exns)                 ;; the index of the exception
    /// ```
    ///
    /// The JavaScript API throws an exception object into a guest as the
    /// exception it wraps, so the handle names the exception the guest
    /// threw, with its tag and its payload.
    pub fn root(&mut self, exception: &JsValue) -> Result<ExnRef> {
        let root = match &self.catcher {
            Some(catcher) => catcher.root.clone(),
            None => {
                let rethrow = Closure::<dyn Fn(JsValue) -> core::result::Result<(), JsValue>>::new(
                    |exception| Err(exception),
                );
                let bytes = wcmp_macros::wasm!(
                    r#"
                    (module
                      (import "" "exns" (table $exns 0 exnref))
                      (import "" "rethrow" (func $rethrow (param externref)))
                      (func (export "root") (param externref) (result i32)
                        block $caught (result exnref)
                          try_table (catch_all_ref $caught)
                            local.get 0
                            call $rethrow
                          end
                          i32.const -1
                          return
                        end
                        i32.const 1
                        table.grow $exns))
                    "#
                );
                let exns = self.exns()?.clone();
                let imports =
                    js::object(&[("exns", exns.into()), ("rethrow", rethrow.as_ref().clone())])
                        .and_then(|imports| js::object(&[("", imports.into())]))
                        .map_err(|error| errors::backend(errors::message(&error)))?;
                let exports = instantiate(bytes, &imports)?;
                let root = js::get(&exports, "root")
                    .ok()
                    .and_then(|root| root.dyn_into::<Function>().ok())
                    .ok_or_else(|| errors::backend("the catcher exports no function"))?;
                self.catcher = Some(Catcher {
                    root: root.clone(),
                    _rethrow: rethrow,
                });
                root
            }
        };
        let index = root
            .call1(&JsValue::UNDEFINED, exception)
            .map_err(|error| errors::backend(errors::message(&error)))?
            .as_f64()
            .filter(|index| *index >= 0.0)
            .ok_or_else(|| errors::backend("the table of exceptions did not grow"))?;
        Ok(ExnRef::from_raw(self.id, index as u64))
    }

    /// Whether a call of a function of type `ty` needs a carrier.
    ///
    /// A `v128` or an `exnref` always does. A float does where the carrier
    /// can name every type of the function: beside a reference to a
    /// concrete type, which no carrier can name, a float crosses as a
    /// `Number`.
    pub fn needed(ty: &FuncType) -> bool {
        let mut slots = ty.params().iter().chain(ty.results()).map(slot);
        let float = slots
            .clone()
            .any(|slot| matches!(slot, Some(Slot::F32 | Slot::F64)));
        required(ty) || (float && slots.all(|slot| slot.is_some()))
    }

    /// The carrier function of the guest function `func`, whose handle has
    /// index `index` in `objects` and whose type is `ty`, and the arguments
    /// it takes for `params`.
    ///
    /// The store calls the carrier with the arguments, and reads the
    /// results with [`Carrier::results`]. The engine already checked the
    /// number and the kinds of the values.
    ///
    /// The carrier imports the function with the type it declares from
    /// `ty`, which is final and alone in its recursion group. A function of
    /// a type that is not final, or of another recursion group, does not
    /// link to it. Where the type holds a float and nothing else a carrier
    /// must carry, the call then goes without a carrier, the floats as a
    /// `Number`, and this returns `None`, for this and every later call of
    /// the function.
    pub fn arguments(
        &mut self,
        objects: &Objects,
        (index, func): (u64, &Function),
        ty: &FuncType,
        params: &[Val],
    ) -> Result<Option<(Function, Array)>> {
        let Some(carrier) = self.function(index, func, ty)? else {
            return Ok(None);
        };
        let mut args = Vec::new();
        for (value, ty) in params.iter().zip(ty.params()) {
            match (slot(ty), value) {
                (Some(Slot::F32), Val::F32(bits)) => {
                    args.push(JsValue::from_f64(f64::from(*bits as i32)));
                }
                (Some(Slot::F64), Val::F64(bits)) => args.push(JsValue::from(*bits as i64)),
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
        Ok(Some((carrier, args.into_iter().collect())))
    }

    /// Writes to `results` the results of a function of type `ty` that
    /// its carrier returned as `returned`.
    pub fn results(
        &self,
        objects: &mut Objects,
        types: &TypeRegistry,
        ty: &FuncType,
        returned: JsValue,
        results: &mut [Val],
    ) -> Result<()> {
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
                Some(Slot::F32) => {
                    let value = next()?;
                    let bits = value.as_f64().ok_or_else(|| {
                        values::mismatch(format!("{value:?} is not the bits of an f32"))
                    })?;
                    Val::F32(bits as i32 as u32)
                }
                Some(Slot::F64) => Val::F64(bits64(next()?, "an f64")?),
                Some(Slot::V128) => {
                    let low = bits64(next()?, "a half of a v128")?;
                    let high = bits64(next()?, "a half of a v128")?;
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
    pub fn exn_index(&mut self, exn: Option<ExnRef>) -> Result<f64> {
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
    pub fn exns(&mut self) -> Result<&WebAssembly::Table> {
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
    /// index `index`, made the first time the host calls it, or `None`
    /// where the function goes without one: see [`Carrier::arguments`].
    fn function(&mut self, index: u64, func: &Function, ty: &FuncType) -> Result<Option<Function>> {
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
        let exports = match WebAssembly::Instance::new(&module, &imports_object) {
            Ok(instance) => instance.exports(),
            Err(error) if goes_without(&error, ty) => {
                self.functions.insert(index, None);
                return Ok(None);
            }
            Err(error) => return Err(errors::backend(errors::message(&error))),
        };
        let carrier = js::get(&exports, "call")
            .ok()
            .and_then(|carrier| carrier.dyn_into::<Function>().ok())
            .ok_or_else(|| errors::backend("the carrier exports no function"))?;
        self.functions.insert(index, Some(carrier.clone()));
        Ok(Some(carrier))
    }
}

/// Whether a function of type `ty` holds a value that the JavaScript API
/// cannot carry at all, a `v128` or an `exnref`, so that a call of it needs
/// a carrier.
fn required(ty: &FuncType) -> bool {
    ty.params()
        .iter()
        .chain(ty.results())
        .any(|ty| matches!(slot(ty), Some(Slot::V128 | Slot::Exn { .. } | Slot::NoExn)))
}

/// The types with which the JavaScript API calls a function of type `ty`,
/// its parameters and its results: those of its carrier where `through`,
/// the call goes through one, and otherwise the function's own. `None`
/// where a generated module cannot name one of them: a reference to a
/// concrete type.
pub fn seen(
    ty: &FuncType,
    through: bool,
) -> Option<(Vec<wasm_encoder::ValType>, Vec<wasm_encoder::ValType>)> {
    if through {
        return Some((
            ty.params().iter().flat_map(carried).collect(),
            ty.results().iter().flat_map(carried).collect(),
        ));
    }
    let own = |types: &[ValType]| {
        types
            .iter()
            .map(|ty| match slot(ty)? {
                Slot::Plain(ty) => Some(ty),
                Slot::F32 => Some(wasm_encoder::ValType::F32),
                Slot::F64 => Some(wasm_encoder::ValType::F64),
                Slot::V128 | Slot::Exn { .. } | Slot::NoExn => None,
            })
            .collect::<Option<Vec<_>>>()
    };
    Some((own(ty.params())?, own(ty.results())?))
}

/// Whether a call of a function of type `ty` goes without a carrier, where
/// the engine refused the carrier with `error`.
///
/// Only a `LinkError` says that the function does not link to the type the
/// carrier imports it with, and only a function whose type holds nothing a
/// carrier must carry can go without one. Every other failure is the
/// backend's.
fn goes_without(error: &JsValue, ty: &FuncType) -> bool {
    !required(ty) && error.is_instance_of::<WebAssembly::LinkError>()
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

/// The 64 bits that the JavaScript API gave as the `BigInt` `value`, which
/// carries `what`: an `f64`, or a half of a `v128`.
fn bits64(value: JsValue, what: &str) -> Result<u64> {
    i64::try_from(value)
        .map(|bits| bits as u64)
        .map_err(|value| values::mismatch(format!("{value:?} is not the bits of {what}")))
}

/// How a carrier takes or gives a value of `ty`, or `None` where no
/// generated module can name `ty`: a reference to a concrete type, whose
/// definition the carrier does not have.
fn slot(ty: &ValType) -> Option<Slot> {
    use wasm_encoder::ValType as Encoded;
    Some(match ty {
        ValType::I32 => Slot::Plain(Encoded::I32),
        ValType::I64 => Slot::Plain(Encoded::I64),
        ValType::F32 => Slot::F32,
        ValType::F64 => Slot::F64,
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
        Slot::F32 => wasm_encoder::ValType::F32,
        Slot::F64 => wasm_encoder::ValType::F64,
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
        Some(Slot::F64) => vec![Encoded::I64],
        Some(Slot::V128) => vec![Encoded::I64, Encoded::I64],
        Some(Slot::F32 | Slot::Exn { .. } | Slot::NoExn) | None => vec![Encoded::I32],
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
            Some(Slot::F32) => {
                code.local_get(local).f32_reinterpret_i32();
                local += 1;
            }
            Some(Slot::F64) => {
                code.local_get(local).f64_reinterpret_i64();
                local += 1;
            }
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
            Some(Slot::F32) => {
                code.local_get(local).i32_reinterpret_f32();
            }
            Some(Slot::F64) => {
                code.local_get(local).i64_reinterpret_f64();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_goes_without_a_carrier_only_where_a_float_function_does_not_link() {
        let float = FuncType::new([ValType::F32], [ValType::F64]);
        let v128 = FuncType::new([ValType::F32], [ValType::V128]);
        let link: JsValue = WebAssembly::LinkError::new("the callee does not link").into();
        assert!(goes_without(&link, &float));
        assert!(!goes_without(&link, &v128));

        // Any other failure of the carrier is the backend's, never a call
        // without a carrier.
        for error in [
            WebAssembly::RuntimeError::new("unreachable").into(),
            WebAssembly::CompileError::new("invalid module").into(),
            js_sys::TypeError::new("callee is not a function").into(),
            js_sys::RangeError::new("out of memory").into(),
            JsValue::from_str("thrown"),
        ] {
            assert!(!goes_without(&error, &float), "{error:?}");
        }
    }
}
