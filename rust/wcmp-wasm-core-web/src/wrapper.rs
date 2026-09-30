//! Generated wrapper modules, through which a guest calls a host function.

use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Function, WebAssembly};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_encoder::{
    AbstractHeapType, BlockType, CodeSection, EntityType, ExportKind, ExportSection,
    FunctionSection, ImportSection, TypeSection,
};
use wcmp_wasm_core::backend::{HostFunc, RawHandle, StoreId};
use wcmp_wasm_core::{Capability, Error, ExnRef, FuncType, HeapType, Result, Val, ValType};

use crate::calls::Calls;
use crate::carrier::Carrier;
use crate::errors;
use crate::js;
use crate::jspi::Jspi;
use crate::objects::Objects;
use crate::type_registry::TypeRegistry;
use crate::values::{self, Kind};

/// The generated wrapper modules of the host functions of one store.
///
/// A JavaScript function that throws into a guest throws an exception,
/// which a guest's `catch_all` catches. A host function that fails must
/// trap the guest instead, and no guest catches a trap. So a guest never
/// calls a JavaScript function directly. It calls the export of a wrapper
/// module, which calls the host through JavaScript functions that never
/// throw:
///
/// ```text
/// (func (export "call") (param ...) (result ...)
///   frame = enter(host)             ;; opens the frame of this call
///   arg(frame, each parameter)      ;; one call for each value
///   if invoke(frame): unreachable   ;; the host function failed: a trap
///   result(frame, each result)      ;; one call for each value
///   leave(frame))                   ;; closes the frame
/// ```
///
/// Each call of a host function has its own frame, so no call shares a
/// buffer with another, and a host function of any number of parameters
/// passes them one at a time. The wrapper is WebAssembly, so it does not
/// break JavaScript Promise Integration, which admits only WebAssembly
/// frames between the start of a stack and a suspension.
///
/// The wrapper of a suspending host function goes on where `invoke`
/// answers that the host function said "not yet" inside a resumable call:
///
/// ```text
///   status = invoke(frame)
///   while status == suspended:
///     suspend(frame)                ;; a `WebAssembly.Suspending` import
///     status = resumed(frame)       ;; suspended again where the call parks
///   if status == failed: unreachable
///   result(frame, each result) ...  ;; the results the host resumed with
/// ```
///
/// The stack suspends inside `suspend` until the host resumes the call, and
/// runs on from there on a microtask. Where the host took the store back in
/// the meantime, the call parks: it suspends again, without reaching the
/// store, until the host waits for it again. The wrapper calls `suspend`
/// only on "not yet", because Chromium suspends on every call of a
/// suspending import, even one whose promise already resolved.
///
/// The JavaScript functions are the store's, and each wrapper instance
/// imports them with the index of its host function as a global. So the
/// store makes one module for each type of host function, suspending or
/// not, and one instance for each host function.
///
/// The JavaScript API carries a float as a `Number`, which can change the
/// bits of a NaN, and carries no `v128` and no `exnref`. So the wrapper
/// carries a float as the integer of its bits, a `v128` as its two `i64`
/// halves, and an `exnref` as its index in the store's table of
/// exceptions, as a carrier does. It carries a result of the internal
/// hierarchy as an `anyref`, and casts it to the result type, so a value of
/// the wrong type traps instead of throwing.
pub struct Wrappers {
    calls: Rc<Calls>,
    jspi: Option<Rc<Jspi>>,
    imports: Option<Imports>,
    modules: HashMap<(FuncType, bool), WebAssembly::Module>,
}

/// The JavaScript functions that each wrapper of a store imports.
///
/// The store keeps each closure for its own life, and the functions fail
/// with the store.
struct Imports {
    enter: Closure<dyn Fn(u32) -> u32>,
    arg: Closure<dyn Fn(u32, JsValue)>,
    invoke: Closure<dyn Fn(u32) -> i32>,
    result: Closure<dyn Fn(u32, u32) -> JsValue>,
    leave: Closure<dyn Fn(u32)>,
    resumed: Closure<dyn Fn(u32) -> i32>,
    suspend: Closure<dyn Fn(u32) -> JsValue>,
    /// `suspend` as a `WebAssembly.Suspending` import, made the first
    /// time a suspending host function needs it.
    suspending: Option<JsValue>,
}

/// How a wrapper carries one value between the guest and the host.
#[derive(Clone, Copy)]
enum Carry {
    /// As it is: an `i32`, an `i64`, or a reference of the function or the
    /// external hierarchy.
    Same(wasm_encoder::ValType),
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
    /// A reference of the internal hierarchy: as it is to the host, and as
    /// an `anyref` that the wrapper casts back to the type from the host.
    Internal(wasm_encoder::RefType),
}

impl Wrappers {
    /// The wrappers of the store whose host functions share `calls`, none
    /// yet. `jspi` makes the wrappers of suspending host functions, where
    /// the browser has it.
    pub fn new(calls: Rc<Calls>, jspi: Option<Rc<Jspi>>) -> Self {
        Self {
            calls,
            jspi,
            imports: None,
            modules: HashMap::new(),
        }
    }

    /// The function that a guest imports for the host function `func` of
    /// type `ty`: the export of a new wrapper instance.
    ///
    /// `carrier` holds the store's table of exceptions, which the wrapper
    /// imports where `ty` has an `exnref`. A suspending host function
    /// needs JavaScript Promise Integration, and is
    /// [`Error::Unsupported`](wcmp_wasm_core::Error::Unsupported) without
    /// it.
    pub fn make(
        &mut self,
        ty: &FuncType,
        func: HostFunc,
        carrier: &mut Carrier,
    ) -> Result<Function> {
        let suspending = func.is_suspending();
        let carries = ty
            .params()
            .iter()
            .chain(ty.results())
            .map(carry)
            .collect::<Result<Vec<_>>>()?;
        let uses_exns = carries
            .iter()
            .any(|carry| matches!(carry, Carry::Exn { .. } | Carry::NoExn));
        let suspend = match (suspending, &self.jspi) {
            (false, _) => None,
            (true, Some(jspi)) => Some(jspi.clone()),
            (true, None) => return Err(Error::Unsupported(Capability::HostSuspension)),
        };
        let key = (ty.clone(), suspending);
        let module = match self.modules.get(&key) {
            Some(module) => module.clone(),
            None => {
                let bytes = generate(ty, uses_exns, suspending)?;
                let module = WebAssembly::Module::new(&js_sys::Uint8Array::from(&bytes[..]).into())
                    .map_err(|error| errors::backend(errors::message(&error)))?;
                self.modules.insert(key, module.clone());
                module
            }
        };
        let index = self.calls.add(ty.clone(), func);
        let imports = self.imports();
        let mut entries = vec![
            ("enter", imports.enter.as_ref().clone()),
            ("arg", imports.arg.as_ref().clone()),
            ("invoke", imports.invoke.as_ref().clone()),
            ("result", imports.result.as_ref().clone()),
            ("leave", imports.leave.as_ref().clone()),
            ("host", JsValue::from_f64(f64::from(index))),
        ];
        if let Some(jspi) = suspend {
            let suspending = match &imports.suspending {
                Some(suspending) => suspending.clone(),
                None => {
                    let suspending = jspi.suspending(imports.suspend.as_ref().unchecked_ref())?;
                    imports.suspending = Some(suspending.clone());
                    suspending
                }
            };
            entries.push(("suspend", suspending));
            entries.push(("resumed", imports.resumed.as_ref().clone()));
        }
        let namespace = js::object(&entries).map_err(|error| errors::call(&error))?;
        if uses_exns {
            js::set(&namespace, "exns", carrier.exns()?).map_err(|error| errors::call(&error))?;
        }
        let imports =
            js::object(&[("", namespace.into())]).map_err(|error| errors::call(&error))?;
        let exports = WebAssembly::Instance::new(&module, &imports)
            .map_err(|error| errors::backend(errors::message(&error)))?
            .exports();
        js::get(&exports, "call")
            .ok()
            .and_then(|call| call.dyn_into::<Function>().ok())
            .ok_or_else(|| errors::backend("the wrapper exports no function"))
    }

    /// The JavaScript functions of the store, made the first time a host
    /// function needs them.
    fn imports(&mut self) -> &mut Imports {
        let calls = &self.calls;
        self.imports.get_or_insert_with(|| {
            let (enter, arg, invoke, result, leave, resumed, suspend) = (
                calls.clone(),
                calls.clone(),
                calls.clone(),
                calls.clone(),
                calls.clone(),
                calls.clone(),
                calls.clone(),
            );
            Imports {
                enter: Closure::new(move |func: u32| enter.open(func)),
                arg: Closure::new(move |frame: u32, value: JsValue| arg.arg(frame, value)),
                invoke: Closure::new(move |frame: u32| invoke.invoke(frame)),
                result: Closure::new(move |frame: u32, index: u32| result.result(frame, index)),
                leave: Closure::new(move |frame: u32| leave.close(frame)),
                resumed: Closure::new(move |frame: u32| resumed.resumed(frame)),
                suspend: Closure::new(move |frame: u32| suspend.suspend(frame)),
                suspending: None,
            }
        })
    }
}

/// The arguments `args` of a call of a host function of type `ty`, as the
/// wrapper carried them, as values of the store `id`.
///
/// A reference that crosses to the host takes a handle in `objects`, which
/// roots it for the life of the store.
pub fn params(
    objects: &mut Objects,
    types: &TypeRegistry,
    id: StoreId,
    ty: &FuncType,
    args: Vec<JsValue>,
) -> Result<Vec<Val>> {
    let width: usize = ty
        .params()
        .iter()
        .map(|ty| carry(ty).map(Carry::width))
        .sum::<Result<_>>()?;
    if args.len() != width {
        return Err(values::mismatch(format!(
            "the wrapper carried {} values for {width}",
            args.len()
        )));
    }
    let mut args = args.into_iter();
    let mut next = || {
        args.next()
            .ok_or_else(|| values::mismatch("the wrapper carried too few values".to_string()))
    };
    ty.params()
        .iter()
        .map(|ty| {
            Ok(match carry(ty)? {
                Carry::F32 => Val::F32(number(&next()?)? as u32),
                Carry::F64 => Val::F64(big(next()?)?),
                Carry::V128 => {
                    let low = big(next()?)?;
                    let high = big(next()?)?;
                    Val::V128(u128::from(low) | (u128::from(high) << 64))
                }
                Carry::Exn { .. } | Carry::NoExn => {
                    let index = number(&next()?)?;
                    Val::ExnRef(
                        u64::try_from(index)
                            .ok()
                            .map(|index| ExnRef::from_raw(id, index)),
                    )
                }
                Carry::Same(_) | Carry::Internal(_) => {
                    values::from_js(objects, next()?, Kind::of_type(ty, types))?
                }
            })
        })
        .collect()
}

/// The results `results` of a host function of type `ty`, as the wrapper
/// carries them.
///
/// Each result must be a value of its type, so that the wrapper converts
/// it without fail: a JavaScript error in the conversion would throw into
/// the guest, which could catch it.
pub fn results(
    objects: &Objects,
    carrier: &mut Carrier,
    types: &TypeRegistry,
    ty: &FuncType,
    results: &[Val],
) -> Result<Vec<JsValue>> {
    let mut carried = Vec::new();
    for (value, ty) in results.iter().zip(ty.results()) {
        check(value, ty, types)?;
        match (carry(ty)?, value) {
            (Carry::F32, Val::F32(bits)) => {
                carried.push(JsValue::from_f64(f64::from(*bits as i32)))
            }
            (Carry::F64, Val::F64(bits)) => carried.push(JsValue::from(*bits as i64)),
            (Carry::V128, Val::V128(bits)) => {
                carried.push(JsValue::from(*bits as u64 as i64));
                carried.push(JsValue::from((*bits >> 64) as u64 as i64));
            }
            (Carry::Exn { .. } | Carry::NoExn, Val::ExnRef(exn)) => {
                carried.push(JsValue::from_f64(carrier.exn_index(*exn)?));
            }
            (Carry::Exn { .. } | Carry::NoExn, _) => carried.push(JsValue::from_f64(-1.0)),
            (_, value) => carried.push(values::to_js(objects, value)?),
        }
    }
    Ok(carried)
}

/// [`Error::TypeMismatch`] where `value` is not a result of type `ty`.
///
/// Beyond the check of [`values::check`], a reference that is not nullable
/// must not be null, and a reference to a bottom type must be.
fn check(value: &Val, ty: &ValType, types: &TypeRegistry) -> Result<()> {
    values::check(value, ty, types)?;
    let Some(ref_type) = ty.ref_type() else {
        return Ok(());
    };
    let bottom = matches!(
        ref_type.heap,
        HeapType::NoFunc | HeapType::NoExtern | HeapType::None | HeapType::NoExn | HeapType::NoCont
    );
    if (value.is_null() && !ref_type.nullable) || (!value.is_null() && bottom) {
        return Err(values::mismatch(format!(
            "{value:?} is not a value of type {ty}"
        )));
    }
    Ok(())
}

/// How a wrapper carries a value of `ty`.
///
/// A generated module cannot name a concrete type, whose definition it
/// does not have, and the JavaScript API carries no continuation
/// reference. So a host function with either in its type is
/// [`Error::Backend`].
fn carry(ty: &ValType) -> Result<Carry> {
    use wasm_encoder::ValType as Encoded;
    Ok(match ty {
        ValType::I32 => Carry::Same(Encoded::I32),
        ValType::I64 => Carry::Same(Encoded::I64),
        ValType::F32 => Carry::F32,
        ValType::F64 => Carry::F64,
        ValType::V128 => Carry::V128,
        ValType::Ref(ty) => {
            let (abstract_type, internal) = match ty.heap {
                HeapType::Exn => {
                    return Ok(Carry::Exn {
                        nullable: ty.nullable,
                    });
                }
                HeapType::NoExn => return Ok(Carry::NoExn),
                HeapType::Func => (AbstractHeapType::Func, false),
                HeapType::NoFunc => (AbstractHeapType::NoFunc, false),
                HeapType::Extern => (AbstractHeapType::Extern, false),
                HeapType::NoExtern => (AbstractHeapType::NoExtern, false),
                HeapType::Any => (AbstractHeapType::Any, true),
                HeapType::Eq => (AbstractHeapType::Eq, true),
                HeapType::I31 => (AbstractHeapType::I31, true),
                HeapType::Struct => (AbstractHeapType::Struct, true),
                HeapType::Array => (AbstractHeapType::Array, true),
                HeapType::None => (AbstractHeapType::None, true),
                HeapType::Cont | HeapType::NoCont => {
                    return Err(errors::backend(
                        "the WebAssembly JavaScript API cannot carry a continuation reference \
                         between the host and a guest",
                    ));
                }
                HeapType::Concrete(_) => {
                    return Err(errors::backend(
                        "the browser backend makes a host function only of abstract reference \
                         types: its wrapper module cannot name a concrete type",
                    ));
                }
            };
            let ty = wasm_encoder::RefType {
                nullable: ty.nullable,
                heap_type: wasm_encoder::HeapType::Abstract {
                    shared: false,
                    ty: abstract_type,
                },
            };
            if internal {
                Carry::Internal(ty)
            } else {
                Carry::Same(Encoded::Ref(ty))
            }
        }
    })
}

impl Carry {
    /// The type of the value inside the wrapper, where the guest passes or
    /// receives it.
    fn encoded(self) -> wasm_encoder::ValType {
        use wasm_encoder::ValType as Encoded;
        match self {
            Carry::Same(ty) => ty,
            Carry::F32 => Encoded::F32,
            Carry::F64 => Encoded::F64,
            Carry::V128 => Encoded::V128,
            Carry::Exn { nullable } => Encoded::Ref(wasm_encoder::RefType {
                nullable,
                heap_type: abstract_heap(AbstractHeapType::Exn),
            }),
            Carry::NoExn => Encoded::Ref(wasm_encoder::RefType {
                nullable: true,
                heap_type: abstract_heap(AbstractHeapType::NoExn),
            }),
            Carry::Internal(ty) => Encoded::Ref(ty),
        }
    }

    /// The number of values the wrapper carries for one value.
    fn width(self) -> usize {
        match self {
            Carry::V128 => 2,
            _ => 1,
        }
    }

    /// The type of each value the wrapper hands to `arg` for a parameter.
    fn arg(self) -> wasm_encoder::ValType {
        use wasm_encoder::ValType as Encoded;
        match self {
            Carry::Same(ty) => ty,
            Carry::Internal(ty) => Encoded::Ref(ty),
            Carry::F32 | Carry::Exn { .. } | Carry::NoExn => Encoded::I32,
            Carry::F64 | Carry::V128 => Encoded::I64,
        }
    }

    /// The type of each value the wrapper takes from `result` for a
    /// result.
    fn result(self) -> wasm_encoder::ValType {
        match self {
            Carry::Internal(_) => wasm_encoder::ValType::Ref(wasm_encoder::RefType::ANYREF),
            carry => carry.arg(),
        }
    }
}

/// The abstract heap type `ty`, unshared.
fn abstract_heap(ty: AbstractHeapType) -> wasm_encoder::HeapType {
    wasm_encoder::HeapType::Abstract { shared: false, ty }
}

/// The `Number` `value` as an `i32`.
fn number(value: &JsValue) -> Result<i32> {
    value
        .as_f64()
        .map(|value| value as i32)
        .ok_or_else(|| values::mismatch(format!("{value:?} is not a number")))
}

/// The `BigInt` `value` as the bits of a `u64`.
fn big(value: JsValue) -> Result<u64> {
    i64::try_from(value)
        .map(|value| value as u64)
        .map_err(|value| values::mismatch(format!("{value:?} is not a BigInt")))
}

/// Adds `ty` to `types` where it is not yet there.
fn join(types: &mut Vec<wasm_encoder::ValType>, ty: wasm_encoder::ValType) {
    if !types.contains(&ty) {
        types.push(ty);
    }
}

/// The index of `ty` in `types`, which [`join`] added it to.
fn position(types: &[wasm_encoder::ValType], ty: wasm_encoder::ValType) -> u32 {
    types.iter().position(|known| *known == ty).unwrap_or(0) as u32
}

/// The wrapper module of a host function of type `ty`:
///
/// ```text
/// (module
///   (import "" "enter" (func $enter (param i32) (result i32)))
///   (import "" "invoke" (func $invoke (param i32) (result i32)))
///   (import "" "leave" (func $leave (param i32)))
///   (import "" "suspend" (func $suspend (param i32)))  ;; where suspending
///   (import "" "resumed" (func $resumed (param i32) (result i32)))
///   (import "" "arg" (func (param i32 T)))            ;; for each carried T
///   (import "" "result" (func (param i32 i32) (result T)))
///   (import "" "host" (global $host i32))
///   (import "" "exns" (table $exns 0 exnref))        ;; where `ty` has one
///   (func (export "call") (type $ty) (local $frame i32) (local $index i32)
///     (local $status i32)
///     ...))
/// ```
///
/// The JavaScript API gives one import of the same name the same function
/// whatever its type, so `arg` and `result` each serve every type.
fn generate(ty: &FuncType, uses_exns: bool, suspending: bool) -> Result<Vec<u8>> {
    use wasm_encoder::ValType as Encoded;
    let params = ty.params().iter().map(carry).collect::<Result<Vec<_>>>()?;
    let results = ty.results().iter().map(carry).collect::<Result<Vec<_>>>()?;
    let mut arg_types = Vec::new();
    for carry in &params {
        join(&mut arg_types, carry.arg());
    }
    let mut result_types = Vec::new();
    for carry in &results {
        join(&mut result_types, carry.result());
    }

    // The types: the wrapper's own, then `enter` and `invoke`, `leave`,
    // each `arg`, and each `result`.
    let mut types = TypeSection::new();
    types.ty().function(
        params.iter().map(|carry| carry.encoded()),
        results.iter().map(|carry| carry.encoded()),
    );
    types.ty().function([Encoded::I32], [Encoded::I32]);
    types.ty().function([Encoded::I32], []);
    for ty in &arg_types {
        types.ty().function([Encoded::I32, *ty], []);
    }
    for ty in &result_types {
        types.ty().function([Encoded::I32, Encoded::I32], [*ty]);
    }
    let first_arg_type = 3;
    let first_result_type = first_arg_type + arg_types.len() as u32;

    // The functions: `enter`, `invoke`, `leave`, `suspend` and `resumed`
    // where the host function is suspending, each `arg`, each `result`,
    // then the wrapper's own. `suspend` has the type of `leave`, and
    // `resumed` the type of `invoke`.
    const ENTER: u32 = 0;
    const INVOKE: u32 = 1;
    const LEAVE: u32 = 2;
    const SUSPEND: u32 = 3;
    const RESUMED: u32 = 4;
    let first_arg = if suspending { 5 } else { 3 };
    let first_result = first_arg + arg_types.len() as u32;
    let call = first_result + result_types.len() as u32;

    let mut imports = ImportSection::new();
    imports.import("", "enter", EntityType::Function(1));
    imports.import("", "invoke", EntityType::Function(1));
    imports.import("", "leave", EntityType::Function(2));
    if suspending {
        imports.import("", "suspend", EntityType::Function(2));
        imports.import("", "resumed", EntityType::Function(1));
    }
    for index in 0..arg_types.len() as u32 {
        imports.import("", "arg", EntityType::Function(first_arg_type + index));
    }
    for index in 0..result_types.len() as u32 {
        imports.import(
            "",
            "result",
            EntityType::Function(first_result_type + index),
        );
    }
    imports.import(
        "",
        "host",
        EntityType::Global(wasm_encoder::GlobalType {
            val_type: Encoded::I32,
            mutable: false,
            shared: false,
        }),
    );
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

    let mut module = wasm_encoder::Module::new();
    module.section(&types);
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("call", ExportKind::Func, call);
    module.section(&exports);

    let frame = params.len() as u32;
    let index = frame + 1;
    let status = frame + 2;
    let arg = |carry: Carry| first_arg + position(&arg_types, carry.arg());
    let result = |carry: Carry| first_result + position(&result_types, carry.result());
    let mut body = wasm_encoder::Function::new([(3, Encoded::I32)]);
    let mut code = body.instructions();
    code.global_get(0).call(ENTER).local_set(frame);
    for (local, carry) in params.iter().enumerate() {
        let local = local as u32;
        match *carry {
            Carry::Same(_) | Carry::Internal(_) => {
                code.local_get(frame).local_get(local).call(arg(*carry));
            }
            Carry::F32 => {
                code.local_get(frame)
                    .local_get(local)
                    .i32_reinterpret_f32()
                    .call(arg(*carry));
            }
            Carry::F64 => {
                code.local_get(frame)
                    .local_get(local)
                    .i64_reinterpret_f64()
                    .call(arg(*carry));
            }
            Carry::V128 => {
                for lane in 0..2 {
                    code.local_get(frame)
                        .local_get(local)
                        .i64x2_extract_lane(lane)
                        .call(arg(*carry));
                }
            }
            Carry::Exn { .. } => {
                code.local_get(frame)
                    .local_get(local)
                    .ref_is_null()
                    .if_(BlockType::Result(Encoded::I32))
                    .i32_const(-1)
                    .else_()
                    .local_get(local)
                    .i32_const(1)
                    .table_grow(0)
                    .end()
                    .call(arg(*carry));
            }
            Carry::NoExn => {
                code.local_get(frame).i32_const(-1).call(arg(*carry));
            }
        }
    }
    code.local_get(frame).call(INVOKE).local_set(status);
    if suspending {
        // 2: the host function said "not yet" inside a resumable call.
        // `resumed` answers 2 again where the resumed call parks, and the
        // stack waits in `suspend` once more.
        code.local_get(status)
            .i32_const(2)
            .i32_eq()
            .if_(BlockType::Empty)
            .loop_(BlockType::Empty)
            .local_get(frame)
            .call(SUSPEND)
            .local_get(frame)
            .call(RESUMED)
            .local_tee(status)
            .i32_const(2)
            .i32_eq()
            .br_if(0)
            .end()
            .end();
    }
    code.local_get(status)
        .if_(BlockType::Empty)
        .unreachable()
        .end();
    let mut carried = 0;
    for carry in &results {
        let take = |code: &mut wasm_encoder::InstructionSink<'_>, carried: i32| {
            code.local_get(frame)
                .i32_const(carried)
                .call(result(*carry));
        };
        match *carry {
            Carry::Same(_) => take(&mut code, carried),
            Carry::Internal(ty) => {
                take(&mut code, carried);
                if !(ty.nullable && ty.heap_type == abstract_heap(AbstractHeapType::Any)) {
                    if ty.nullable {
                        code.ref_cast_nullable(ty.heap_type);
                    } else {
                        code.ref_cast_non_null(ty.heap_type);
                    }
                }
            }
            Carry::F32 => {
                take(&mut code, carried);
                code.f32_reinterpret_i32();
            }
            Carry::F64 => {
                take(&mut code, carried);
                code.f64_reinterpret_i64();
            }
            Carry::V128 => {
                code.v128_const(0);
                for lane in 0..2 {
                    take(&mut code, carried + i32::from(lane));
                    code.i64x2_replace_lane(lane);
                }
            }
            Carry::Exn { nullable } => {
                take(&mut code, carried);
                code.local_set(index)
                    .local_get(index)
                    .i32_const(-1)
                    .i32_eq()
                    .if_(BlockType::Result(Encoded::EXNREF))
                    .ref_null(abstract_heap(AbstractHeapType::Exn))
                    .else_()
                    .local_get(index)
                    .table_get(0)
                    .end();
                if !nullable {
                    code.ref_as_non_null();
                }
            }
            Carry::NoExn => {
                code.ref_null(abstract_heap(AbstractHeapType::NoExn));
            }
        }
        carried += carry.width() as i32;
    }
    code.local_get(frame).call(LEAVE).end();
    let mut section = CodeSection::new();
    section.function(&body);
    module.section(&section);
    Ok(module.finish())
}
