//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{AsContextMut, Val as RuntimeVal};

use crate::abi::context::{LiftContext, LowerContext};
use crate::abi::flatten::lower_into_flat_slots;
use crate::abi::layout::{FlatType, flat_count, flat_types};
use crate::abi::lift;

/// Per the canonical ABI, the maximum flat-slot count for the
/// parameter tuple before the call switches to the wide-arg memory
/// pointer path.
const MAX_FLAT_PARAMS: usize = 16;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result};
use crate::executor::ir::CanonOptions;
use crate::executor::trampoline::AbiRuntimeState;
use crate::store::Store;
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// A handle to one exported function of a component [`Instance`].
///
/// `Func` is obtained from [`Instance::get_func`] and is the unit a
/// caller invokes through. Calling drives the canonical-ABI
/// round-trip: lifts arguments through the export's canon options
/// (calling `cabi_realloc` for heap-allocating values), passes them
/// to the underlying core function, lowers the result back into the
/// polyfill's [`Val`] enum, and runs the export's `post-return`
/// after the caller observes the return.
///
/// [`Instance`]: super::Instance
/// [`Instance::get_func`]: super::Instance::get_func
pub struct Func {
    /// The leaf name this export was declared under. Carried so
    /// the typed-conversion entry point can name the export in
    /// type-mismatch diagnostics; `Func::call` does not consult it.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub name: String,
    /// The runtime-layer core-Wasm function handle this export
    /// resolves to. Workspace-internal; never re-exported through
    /// `lib.rs`.
    pub inner: wasm_runtime_layer::Func,
    /// The component-level signature the polyfill uses to lower
    /// arguments and lift results across the canonical-ABI
    /// boundary. Workspace-internal; never re-exported through
    /// `lib.rs`.
    pub signature: FunctionType,
    /// The canonical-ABI options the export's lift declared. Held
    /// here so [`Self::call`] can resolve memory/realloc/post-
    /// return at call time.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub options: CanonOptions,
    /// The instance's canonical-ABI runtime state. Shared with
    /// every host trampoline the same instance carries; the lock
    /// is taken briefly at the boundaries.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub abi_state: Arc<Mutex<AbiRuntimeState>>,
}

impl Func {
    /// Invoke the function with the given polyfill-typed arguments.
    /// Drives the full canonical-ABI round-trip: heap-allocating
    /// arguments are lowered into guest memory via `cabi_realloc`,
    /// the underlying core function is called, the result is
    /// lifted, and `post-return` (when declared) is invoked once
    /// the caller has observed the return value.
    ///
    /// `T` is the host-data type of the [`Store`] the instance was
    /// created in.
    pub fn call<T: 'static>(&self, store: &mut Store<T>, args: &[Val]) -> Result<Box<[Val]>> {
        if args.len() != self.signature.parameters.len() {
            return Err(Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: ValueType::Primitive(PrimitiveType::Bool),
                cause: AbiCause::InvalidEncoding {
                    message: format!(
                        "expected {} arguments, got {}",
                        self.signature.parameters.len(),
                        args.len()
                    ),
                },
            }));
        }

        // Resolve memory / realloc / post-return from the instance
        // state for the duration of the call.
        let (memory, realloc, post_return) = {
            let state = self
                .abi_state
                .lock()
                .map_err(|_| internal("ABI runtime state lock poisoned"))?;
            let memory = self
                .options
                .memory
                .and_then(|s| state.memories.get(s).and_then(|m| m.clone()));
            let realloc = self
                .options
                .realloc
                .and_then(|s| state.reallocs.get(s).and_then(|f| f.clone()));
            let post_return = self
                .options
                .post_return
                .and_then(|s| state.post_returns.get(s).and_then(|f| f.clone()));
            (memory, realloc, post_return)
        };

        let core_args = self.lower_args(store, args, memory.clone(), realloc.clone())?;
        let result_arity = self.core_result_arity();
        let mut core_results = vec![RuntimeVal::I32(0); result_arity];

        self.inner
            .call(store.inner_mut(), &core_args, &mut core_results)
            .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)))?;

        let lifted_result = self.lift_result(store, &core_results, memory)?;

        // Run post-return (if any) after the caller has logically
        // observed the return; we hold the lifted value, so the
        // post-return is safe to call now.
        if let Some(post_return_func) = post_return {
            // post-return takes the original core result values as
            // its arguments. Mirror the canonical-ABI rule: when
            // the result fits in MAX_FLAT_RESULTS=1, the core
            // result slot is its argument; when it doesn't, the
            // pointer slot the caller allocated is.
            let mut empty: [RuntimeVal; 0] = [];
            post_return_func
                .call(store.inner_mut(), &core_results, &mut empty)
                .map_err(|err| {
                    Error::from(AbiError {
                        position: AbiPosition::Result,
                        valtype: ValueType::Primitive(PrimitiveType::Bool),
                        cause: AbiCause::SubstrateFailure(err),
                    })
                })?;
        }

        let mut out: Vec<Val> = Vec::with_capacity(usize::from(self.signature.result.is_some()));
        if let Some(val) = lifted_result {
            out.push(val);
        }
        Ok(out.into_boxed_slice())
    }

    /// The number of core-Wasm result slots the underlying core
    /// function returns. Mirrors the rule
    /// [`crate::executor::trampoline`] uses to derive the core
    /// function type from the polyfill's signature.
    fn core_result_arity(&self) -> usize {
        match &self.signature.result {
            None => 0,
            Some(result_ty) => match flat_count(result_ty) {
                Some(n) if n <= 1 => flat_types(result_ty).len(),
                _ => 1, // single result-area pointer
            },
        }
    }

    fn lower_args<T: 'static>(
        &self,
        store: &mut Store<T>,
        args: &[Val],
        memory: Option<wasm_runtime_layer::Memory>,
        realloc: Option<wasm_runtime_layer::Func>,
    ) -> Result<Vec<RuntimeVal>> {
        let tables = store.tables_handle();
        let store_ctx = store.inner_mut().as_context_mut();
        let mut lower_ctx = LowerContext::new(
            store_ctx,
            memory,
            realloc,
            self.options.string_encoding,
            Some(tables),
        );

        // Compute the total flat-slot count for the parameter
        // tuple. The canonical ABI uses per-slot flat passing when
        // it fits in `MAX_FLAT_PARAMS = 16`; otherwise the entire
        // tuple is passed as a single memory pointer.
        let total_flat: Option<usize> = self
            .signature
            .parameters
            .iter()
            .try_fold(0usize, |acc, p| flat_count(&p.ty).map(|n| acc + n));

        match total_flat {
            Some(n) if n <= MAX_FLAT_PARAMS => {
                let mut out: Vec<RuntimeVal> = Vec::new();
                for (i, (param, val)) in self
                    .signature
                    .parameters
                    .iter()
                    .zip(args.iter())
                    .enumerate()
                {
                    lower_into_flat_slots(
                        &mut lower_ctx,
                        val,
                        &param.ty,
                        &mut out,
                        AbiPosition::Argument(i),
                    )?;
                }
                Ok(out)
            }
            _ => Err(Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: ValueType::Primitive(PrimitiveType::Bool),
                cause: AbiCause::InvalidEncoding {
                    message: format!(
                        "lifted-export call has more than {MAX_FLAT_PARAMS} flat parameter slots; the wide-arg memory-pointer path is not yet implemented"
                    ),
                },
            })),
        }
    }

    fn lift_result<T: 'static>(
        &self,
        store: &mut Store<T>,
        core_results: &[RuntimeVal],
        memory: Option<wasm_runtime_layer::Memory>,
    ) -> Result<Option<Val>> {
        let Some(result_ty) = &self.signature.result else {
            return Ok(None);
        };
        let position = AbiPosition::Result;
        let tables = store.tables_handle();
        let store_ctx = store.inner_mut().as_context_mut();
        let mut lift_ctx = LiftContext::new(
            store_ctx,
            memory,
            self.options.string_encoding,
            Some(tables),
        );
        match flat_count(result_ty) {
            Some(n) if n <= 1 => {
                if core_results.is_empty() {
                    return Err(Error::from(AbiError {
                        position,
                        valtype: result_ty.clone(),
                        cause: AbiCause::InvalidEncoding {
                            message: "missing core result slot".to_owned(),
                        },
                    }));
                }
                let val =
                    lift_value_from_flat(&mut lift_ctx, &core_results[0], result_ty, position)?;
                Ok(Some(val))
            }
            _ => {
                // Wide result: read from the pointer the core
                // function returned.
                let ptr = match core_results.first() {
                    Some(RuntimeVal::I32(p)) => *p as usize,
                    _ => {
                        return Err(Error::from(AbiError {
                            position,
                            valtype: result_ty.clone(),
                            cause: AbiCause::InvalidEncoding {
                                message: "missing or non-i32 result-pointer slot".to_owned(),
                            },
                        }));
                    }
                };
                let val = lift(&mut lift_ctx, ptr, result_ty, position)?;
                Ok(Some(val))
            }
        }
    }
}

fn lift_value_from_flat<T: 'static>(
    _ctx: &mut LiftContext<'_, T>,
    core: &RuntimeVal,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<Val> {
    let mismatch = || {
        Error::from(AbiError {
            position,
            valtype: ty.clone(),
            cause: AbiCause::HostValueMismatch,
        })
    };
    match (ty, core) {
        (ValueType::Primitive(PrimitiveType::Bool), RuntimeVal::I32(v)) => Ok(Val::Bool(*v != 0)),
        (ValueType::Primitive(PrimitiveType::S8), RuntimeVal::I32(v)) => Ok(Val::S8(*v as i8)),
        (ValueType::Primitive(PrimitiveType::U8), RuntimeVal::I32(v)) => Ok(Val::U8(*v as u8)),
        (ValueType::Primitive(PrimitiveType::S16), RuntimeVal::I32(v)) => Ok(Val::S16(*v as i16)),
        (ValueType::Primitive(PrimitiveType::U16), RuntimeVal::I32(v)) => Ok(Val::U16(*v as u16)),
        (ValueType::Primitive(PrimitiveType::S32), RuntimeVal::I32(v)) => Ok(Val::S32(*v)),
        (ValueType::Primitive(PrimitiveType::U32), RuntimeVal::I32(v)) => Ok(Val::U32(*v as u32)),
        (ValueType::Primitive(PrimitiveType::S64), RuntimeVal::I64(v)) => Ok(Val::S64(*v)),
        (ValueType::Primitive(PrimitiveType::U64), RuntimeVal::I64(v)) => Ok(Val::U64(*v as u64)),
        (ValueType::Primitive(PrimitiveType::F32), RuntimeVal::F32(v)) => Ok(Val::F32(*v)),
        (ValueType::Primitive(PrimitiveType::F64), RuntimeVal::F64(v)) => Ok(Val::F64(*v)),
        (ValueType::Primitive(PrimitiveType::Char), RuntimeVal::I32(v)) => {
            char::from_u32(*v as u32).map(Val::Char).ok_or_else(|| {
                Error::from(AbiError {
                    position,
                    valtype: ty.clone(),
                    cause: AbiCause::InvalidEncoding {
                        message: "core return is not a valid Unicode scalar for `char`".to_owned(),
                    },
                })
            })
        }
        // Compound types and `string` whose flat count exceeds 1
        // take the wide-result path; reaching here means the
        // signature's flat-count agreed with the single-slot path,
        // which only fits primitives non-string.
        _ => {
            let _ = FlatType::I32;
            Err(mismatch())
        }
    }
}

fn internal(message: &str) -> Error {
    Error::Internal {
        message: message.to_owned(),
    }
}
