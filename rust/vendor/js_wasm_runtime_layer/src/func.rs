use alloc::{vec, vec::Vec};

use anyhow::{Context, Result};
use js_sys::{Array, Function};
use wasm_bindgen::{closure::Closure, prelude::wasm_bindgen, JsCast, JsValue};
use wasm_runtime_layer::{
    backend::{AsContext, AsContextMut, Val, WasmFunc},
    FuncType, ValType,
};

use crate::{
    conversion::ToStoredJs, value_from_js_typed, value_from_js_untyped, DropResource, Engine,
    JsErrorMsg, StoreContextMut, StoreInner,
};

/// A bound function
#[derive(Debug, Clone)]
pub struct Func {
    /// Index
    pub(crate) id: usize,
}

/// Internal representation of [`Func`]
#[derive(Debug)]
pub(crate) struct FuncInner {
    /// The inner Js function
    pub(crate) func: Function,
    /// The function signature
    ty: FuncType,
    /// PATCH (wcmp): whether `ty` is the function's real signature.
    /// A function reference that reached the host as an argument
    /// carries none: the JS API hands over the function object
    /// alone, and nothing in it says what the function takes or
    /// returns. A call to such a function reads its result count
    /// and types off the slice the caller supplied; see
    /// [`WasmFunc::call`] and `value_from_js_untyped`.
    signature_known: bool,
}

impl FuncInner {
    /// PATCH (wcmp): the record of a function reference the host
    /// received as an argument, whose signature the JS API does not
    /// carry.
    pub(crate) fn of_unknown_signature(func: Function) -> Self {
        Self {
            func,
            ty: FuncType::new([], []),
            signature_known: false,
        }
    }
}

impl ToStoredJs for Func {
    type Repr = Function;
    fn to_stored_js<T>(&self, store: &StoreInner<T>) -> Result<Function> {
        let func = &store.funcs[self.id];
        Ok(func.func.clone())
    }
}

impl Func {
    /// Creates a new function from a JS value
    pub fn from_exported_function<T>(
        store: &mut StoreInner<T>,
        value: JsValue,
        signature: FuncType,
    ) -> Option<Self> {
        let func: Function = value.dyn_into().ok()?;

        Some(store.insert_func(FuncInner {
            func,
            // TODO: we don't really know what the exported function's signature is
            ty: signature,
            signature_known: true,
        }))
    }
}

/// Converts any repeated argument to `JsValue`
macro_rules! to_ty {
    ($v: ident) => {
        JsValue
    };
}

/// Creates a variable argument wrapper around a host function
macro_rules! func_wrapper {
    ($store: ident, $func_ty: ident, $func: ident, $($idx: tt => $ident: ident),*) => {{
        let ty = $func_ty.clone();
        // PATCH (wcmp): the shim is a shared closure and not a
        // `FnMut` one. `wasm_bindgen` clears the pointer of a
        // mutable closure for the length of a call and throws
        // "closure invoked recursively or after being dropped" from
        // inside a second one. A shared closure is entered at any
        // depth, and nothing in it needs exclusive access: `$func`
        // is `Fn`, and the arguments and results of a call live in
        // that call's own frame, so a host function already on the
        // stack is entered again as a native engine enters it.
        let closure: Closure<dyn Fn($(to_ty!($ident)),*) -> Result<JsValue, JsValue>> = Closure::new(move |$($ident: JsValue),*| -> Result<JsValue, JsValue> {
            // Safety:
            //
            // This closure is stored inside the store.
            //
            // The closure itself is accessed through a raw pointer, and does not produce any
            // reference to `StoreInner<T>`.
            let store: &mut StoreInner<T> = unsafe { &mut *($store as *mut StoreInner<T>) };
            #[allow(unused_mut)]
            let mut store = StoreContextMut::from_ref(store);

            let _arg_types = ty.params();

            let args = [
                $(
                    (value_from_js_typed(&mut store, &_arg_types[$idx], $ident)).expect("Failed to convert argument"),
                )*
            ];

            match $func(store, &ty, &args) {
                Ok(v) => { Ok(v.into()) }
                Err(err) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!("{err:?}");
                    let message = format!("host function failed: {err:#}");
                    // Keep the host's own error for the outer call; see
                    // `StoreInner::pending_host_error`.
                    let store: &mut StoreInner<T> = unsafe { &mut *($store as *mut StoreInner<T>) };
                    // The first error wins: an adapter that catches this
                    // exception re-traps through the same shim, and that
                    // second error must not replace the cause.
                    if store.pending_host_error.is_none() {
                        store.pending_host_error = Some(err);
                    }
                    Err(js_sys::Error::new(&message).into())
                }
            }
        });

        let func = closure.as_ref().unchecked_ref::<Function>().clone();
        let drop_resource = DropResource::new(closure);

        (drop_resource, func)
    }};
}

/// PATCH (wcmp): the JavaScript half of [`variadic_wrapper`].
///
/// `collect_arguments` answers a function of no declared arity that
/// gathers whatever it was called with into an array and hands the
/// array to `callee`, which is the closure's own function object.
///
/// `wasm_bindgen` writes the snippet to a file beside the module's
/// own glue and the page loads it as ordinary script, so a
/// content-security policy whose `script-src` grants `'self'` and
/// `'wasm-unsafe-eval'` — enough for the module itself, and the
/// common hardened setting — admits it unchanged. Building the same
/// function from source text at runtime, with `js_sys`'s
/// `Function::new_with_args`, would be `new Function`, which such a
/// policy refuses; every call an adapter prepares — which is every
/// call whose lower or lift is asynchronous — then fails on a page
/// that sets one, because a prepared call that passes an argument
/// reaches the arity this wrapper exists for.
#[wasm_bindgen(inline_js = "export function collect_arguments(callee) {
    return function () {
        return callee(Array.prototype.slice.call(arguments));
    };
}
")]
extern "C" {
    /// Wrap `callee` in a function of no declared arity that passes
    /// it an array of the arguments of each call.
    fn collect_arguments(callee: &JsValue) -> Function;
}

/// PATCH (wcmp): the same wrapper, for a host function of more
/// parameters than a `wasm_bindgen` closure can take.
///
/// `Closure::new` is implemented for functions of at most eight
/// arguments, and the prepare-call intrinsic a fused adapter imports
/// takes eight fixed arguments followed by the caller's own flat
/// arguments, which the canonical ABI allows sixteen of. The closure
/// therefore takes one JS array, and a small JS shim collects the
/// call's `arguments` into it, so the guest still sees an ordinary
/// function of the declared arity.
fn variadic_wrapper<T: 'static>(
    store_ptr: *mut (),
    func_ty: FuncType,
    func: impl 'static + Fn(StoreContextMut<T>, &FuncType, &[Val<Engine>]) -> Result<JsValue>,
) -> (DropResource, Function) {
    let ty = func_ty.clone();
    // PATCH (wcmp): a shared closure, entered at any depth, for the
    // reason `func_wrapper!` states.
    let closure: Closure<dyn Fn(Array) -> Result<JsValue, JsValue>> = Closure::new(move |values: Array| -> Result<JsValue, JsValue> {
        // Safety: as in `func_wrapper!`, the closure is stored
        // inside the store and produces no reference to it.
        let store: &mut StoreInner<T> = unsafe { &mut *(store_ptr as *mut StoreInner<T>) };
        let mut store = StoreContextMut::from_ref(store);

        let arg_types = ty.params();
        let mut args = alloc::vec::Vec::with_capacity(arg_types.len());
        for (index, arg_ty) in arg_types.iter().enumerate() {
            match value_from_js_typed(&mut store, arg_ty, values.get(index as u32)) {
                Some(value) => args.push(value),
                None => {
                    return Err(js_sys::Error::new(
                        "host function received an argument it cannot convert",
                    )
                    .into());
                }
            }
        }

        match func(store, &ty, &args) {
            Ok(v) => Ok(v),
            Err(err) => {
                #[cfg(feature = "tracing")]
                tracing::error!("{err:?}");
                let message = format!("host function failed: {err:#}");
                let store: &mut StoreInner<T> = unsafe { &mut *(store_ptr as *mut StoreInner<T>) };
                if store.pending_host_error.is_none() {
                    store.pending_host_error = Some(err);
                }
                Err(js_sys::Error::new(&message).into())
            }
        }
    });

    let func = collect_arguments(closure.as_ref());
    (DropResource::new(closure), func)
}

impl WasmFunc<Engine> for Func {
    fn new<T: 'static>(
        mut ctx: impl AsContextMut<Engine, UserState = T>,
        ty: FuncType,
        func: impl 'static
            + Send
            + Sync
            + Fn(StoreContextMut<T>, &[Val<Engine>], &mut [Val<Engine>]) -> Result<()>,
    ) -> Self {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("Func::new").entered();

        let mut ctx: StoreContextMut<_> = ctx.as_context_mut();

        // Keep a reference to the store to calling context to reconstruct it later during export
        // call.
        //
        // The allocated closure which uses this pointer is stored in the store itself, and as such
        // dropping the store will also drop and prevent any further use of this pointer.
        //
        // The pointer itself is allocated on the stack and will *never* be moved during the entire
        // lifetime of the store.
        //
        // See: [`crate::web::store::Store`] for more details of why this is done this way.
        let store_ptr = ctx.as_ptr();

        // Remove `T` from this pointer and untie the lifetime.
        //
        // Lifetime is enforced by the closured storage in the store, and Store is guaranteed to
        // live as long as this closure
        let store_ptr = store_ptr as *mut ();

        // PATCH (wcmp): each call gets a results buffer of its own.
        // Upstream allocates one buffer per host function and
        // captures it here, which makes the body `FnMut` and lets a
        // second call of the same function write over the results of
        // a first one still on the stack.
        let result_count = ty.results().len();

        let func = {
            move |mut store: StoreContextMut<T>, _ty: &FuncType, args: &[Val<Engine>]| {
                #[cfg(feature = "tracing")]
                let _span = tracing::debug_span!("call_host", ty=%_ty, ?args).entered();

                let mut res = vec![Val::I32(0); result_count];

                match func(store.as_context_mut(), args, &mut res) {
                    Ok(()) => {
                        #[cfg(feature = "tracing")]
                        tracing::debug!(?res, "result");
                    }
                    Err(err) => {
                        #[cfg(feature = "tracing")]
                        tracing::error!("{err:?}");
                        return Err(err);
                    }
                };

                let results = match &res[..] {
                    [] => JsValue::UNDEFINED,
                    [res] => res.to_stored_js(&*store)?,
                    res => res
                        .iter()
                        .map(|v| v.to_stored_js(&*store))
                        .collect::<Result<Array>>()?
                        .into(),
                };

                Ok(results)
            }
        };

        let (resource, func) = match ty.params().len() {
            0 => func_wrapper!(store_ptr, ty, func,),
            1 => func_wrapper!(store_ptr, ty, func, 0 => a),
            2 => func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b),
            3 => func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b, 2 => c),
            4 => func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b, 2 => c, 3 => d),
            5 => func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b, 2 => c, 3 => d, 4 => e),
            6 => func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b, 2 => c, 3 => d, 4 => e, 5 => f),
            7 => {
                func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b, 2 => c, 3 => d, 4 => e, 5 => f, 6 => g)
            }
            8 => {
                func_wrapper!(store_ptr, ty, func, 0 => a, 1 => b, 2 => c, 3 => d, 4 => e, 5 => f, 6 => g, 7 => h)
            }
            // PATCH (wcmp): a host function of more parameters than a
            // `wasm_bindgen` closure can take collects them from one
            // JS array instead; see `variadic_wrapper`.
            _ => variadic_wrapper(store_ptr, ty.clone(), func),
        };

        let func = ctx.insert_func(FuncInner {
            func,
            ty,
            signature_known: true,
        });

        #[cfg(feature = "tracing")]
        tracing::debug!(id = func.id, "func");
        ctx.insert_drop_resource(DropResource::new(resource));

        func
    }

    fn ty(&self, ctx: impl AsContext<Engine>) -> FuncType {
        ctx.as_context().funcs[self.id].ty.clone()
    }

    fn call<T>(
        &self,
        mut ctx: impl AsContextMut<Engine>,
        args: &[Val<Engine>],
        results: &mut [Val<Engine>],
    ) -> Result<()> {
        let ctx: &mut StoreInner<_> = &mut *ctx.as_context_mut();
        let inner: &FuncInner = &ctx.funcs[self.id];
        let func = inner.func.clone();
        // PATCH (wcmp): a function reference the host received as an
        // argument carries no signature, so the caller's own result
        // slice says how many values come back and what they are;
        // see `value_from_js_untyped` for the one rule on top of it.
        let signature_known = inner.signature_known;
        let result_types: Vec<ValType> = if signature_known {
            inner.ty.results().to_vec()
        } else {
            results.iter().map(|value| value.ty()).collect()
        };
        let ty = inner.ty.clone();

        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("call_guest", ?args, %ty).entered();

        let args = args
            .iter()
            .map(|v| v.to_stored_js(ctx))
            .collect::<Result<Array>>()?;

        // PATCH (wcmp): a failed call reports the host's own error when a
        // host function failed during it; see `StoreInner::pending_host_error`.
        let res = match func.apply(&JsValue::UNDEFINED, &args) {
            Ok(res) => {
                ctx.pending_host_error = None;
                res
            }
            Err(js_error) => {
                return Err(match ctx.pending_host_error.take() {
                    Some(err) => err.context("Guest function threw an error"),
                    None => anyhow::Error::from(JsErrorMsg::from(js_error))
                        .context("Guest function threw an error"),
                });
            }
        };

        #[cfg(feature = "tracing")]
        tracing::debug!(?res,ty=?inner.ty);

        // https://webassembly.github.io/spec/js-api/#exported-function-exotic-objects
        assert_eq!(result_types.len(), results.len());
        match &result_types[..] {
            // void
            [] => {}
            // single
            &[ty] => {
                results[0] = if signature_known {
                    value_from_js_typed(ctx, &ty, res).context("Failed to convert return value")?
                } else {
                    value_from_js_untyped(ctx, &ty, res).context("Failed to convert return value")?
                };
            }
            // multi-value
            tys => {
                for ((src, ty), dst) in res
                    .dyn_into::<Array>()
                    .map_err(JsErrorMsg::from)
                    .context("Failed to convert return value to array")?
                    .iter()
                    .zip(tys)
                    .zip(results)
                {
                    *dst = if signature_known {
                        value_from_js_typed(ctx, ty, src.clone())
                            .context("Failed to convert return value")?
                    } else {
                        value_from_js_untyped(ctx, ty, src.clone())
                            .context("Failed to convert return value")?
                    };
                }
            }
        }

        Ok(())
    }
}
