//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use crate::component::FunctionType;
use crate::error::{Error, InstantiationError, Result};
use crate::store::Store;
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// A handle to one exported function of a component [`Instance`].
///
/// `Func` is obtained from [`Instance::get_func`] and is the unit a
/// caller invokes through. Calling drives the canonical-ABI
/// round-trip that lifts arguments, passes them to the underlying
/// core function, and lowers the result back into the polyfill's
/// [`Val`] enum.
///
/// At present only primitive valtypes are supported on either side
/// of the call. A function whose signature requires compound-
/// valtype lift/lower surfaces a [`crate::Error::Instantiation`]
/// before the call is attempted.
///
/// The substrate handle is the runtime-layer's core-Wasm `Func`
/// (the polyfill's component executor wires component-level calls
/// down to one or more core-Wasm calls); the field is workspace-
/// internal and never reaches the public API.
///
/// [`Instance`]: super::Instance
/// [`Instance::get_func`]: super::Instance::get_func
pub struct Func {
    /// The runtime-layer core-Wasm function handle this export
    /// resolves to. Workspace-internal; never re-exported through
    /// `lib.rs`.
    pub inner: wasm_runtime_layer::Func,
    /// The component-level signature the polyfill uses to lower
    /// arguments and lift results across the canonical-ABI
    /// boundary. Workspace-internal; never re-exported through
    /// `lib.rs`.
    pub signature: FunctionType,
}

impl Func {
    /// Invoke the function with the given primitive-valtyped
    /// arguments. The call is synchronous; the polyfill's return
    /// type is `Box<[Val]>` so that future work introducing multi-
    /// result calls lands without reshaping the public API.
    ///
    /// `T` is the host-data type of the [`Store`] the instance was
    /// created in.
    pub fn call<T: 'static>(&self, store: &mut Store<T>, args: &[Val]) -> Result<Box<[Val]>> {
        assert_eq!(
            args.len(),
            self.signature.parameters.len(),
            "argument count does not match the function's signature",
        );
        let core_args: Vec<wasm_runtime_layer::Val> = args
            .iter()
            .zip(self.signature.parameters.iter())
            .map(|(arg, param)| lower_primitive(arg, &param.ty))
            .collect();

        let result_arity = usize::from(self.signature.result.is_some());
        let mut core_results = vec![wasm_runtime_layer::Val::I32(0); result_arity];

        self.inner
            .call(store.inner_mut(), &core_args, &mut core_results)
            .map_err(|err| Error::Instantiation(InstantiationError::SubstrateFailure(err)))?;

        let mut results = Vec::with_capacity(result_arity);
        if let Some(result_ty) = &self.signature.result {
            results.push(lift_primitive(&core_results[0], result_ty));
        }
        Ok(results.into_boxed_slice())
    }
}

/// Lower a polyfill primitive [`Val`] to the runtime-layer core
/// [`wasm_runtime_layer::Val`] expected by the underlying core
/// function. The component-level value type the call site expects
/// is supplied so the lower can validate the variants line up.
fn lower_primitive(val: &Val, expected: &ValueType) -> wasm_runtime_layer::Val {
    let ValueType::Primitive(prim) = expected else {
        todo!(
            "compound-valtype lift/lower (records, variants, lists, options, results, tuples, flags, enums, strings, resource handles) lands with PDD008"
        )
    };
    match (val, prim) {
        (Val::Bool(b), PrimitiveType::Bool) => wasm_runtime_layer::Val::I32(i32::from(*b)),
        (Val::S8(v), PrimitiveType::S8) => wasm_runtime_layer::Val::I32(i32::from(*v)),
        (Val::U8(v), PrimitiveType::U8) => wasm_runtime_layer::Val::I32(i32::from(*v)),
        (Val::S16(v), PrimitiveType::S16) => wasm_runtime_layer::Val::I32(i32::from(*v)),
        (Val::U16(v), PrimitiveType::U16) => wasm_runtime_layer::Val::I32(i32::from(*v)),
        (Val::S32(v), PrimitiveType::S32) => wasm_runtime_layer::Val::I32(*v),
        (Val::U32(v), PrimitiveType::U32) => wasm_runtime_layer::Val::I32(*v as i32),
        (Val::S64(v), PrimitiveType::S64) => wasm_runtime_layer::Val::I64(*v),
        (Val::U64(v), PrimitiveType::U64) => wasm_runtime_layer::Val::I64(*v as i64),
        (Val::F32(v), PrimitiveType::F32) => wasm_runtime_layer::Val::F32(*v),
        (Val::F64(v), PrimitiveType::F64) => wasm_runtime_layer::Val::F64(*v),
        (Val::Char(c), PrimitiveType::Char) => wasm_runtime_layer::Val::I32(*c as i32),
        (_, PrimitiveType::String) => {
            todo!("string lift/lower lands with PDD008 (the canonical ABI runtime state)")
        }
        _ => panic!(
            "argument variant {val:?} does not match the parameter's primitive type {prim:?}"
        ),
    }
}

/// Lift a runtime-layer core [`wasm_runtime_layer::Val`] returned
/// from the underlying core function to a polyfill primitive
/// [`Val`] of the component-level result type.
fn lift_primitive(core: &wasm_runtime_layer::Val, expected: &ValueType) -> Val {
    let ValueType::Primitive(prim) = expected else {
        todo!(
            "compound-valtype lift/lower (records, variants, lists, options, results, tuples, flags, enums, strings, resource handles) lands with PDD008"
        )
    };
    match (core, prim) {
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::Bool) => Val::Bool(*v != 0),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::S8) => Val::S8(*v as i8),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::U8) => Val::U8(*v as u8),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::S16) => Val::S16(*v as i16),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::U16) => Val::U16(*v as u16),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::S32) => Val::S32(*v),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::U32) => Val::U32(*v as u32),
        (wasm_runtime_layer::Val::I64(v), PrimitiveType::S64) => Val::S64(*v),
        (wasm_runtime_layer::Val::I64(v), PrimitiveType::U64) => Val::U64(*v as u64),
        (wasm_runtime_layer::Val::F32(v), PrimitiveType::F32) => Val::F32(*v),
        (wasm_runtime_layer::Val::F64(v), PrimitiveType::F64) => Val::F64(*v),
        (wasm_runtime_layer::Val::I32(v), PrimitiveType::Char) => char::from_u32(*v as u32)
            .map(Val::Char)
            .expect("primitive lift: core i32 must be a valid Unicode scalar for `char`"),
        (_, PrimitiveType::String) => {
            todo!("string lift/lower lands with PDD008 (the canonical ABI runtime state)")
        }
        _ => panic!(
            "core return variant {core:?} does not match the result's primitive type {prim:?}"
        ),
    }
}
