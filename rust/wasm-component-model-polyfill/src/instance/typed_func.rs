//! A typed handle for invoking a component export with native Rust
//! values.
//!
//! `TypedFunc<P, R>` is the typed counterpart to [`Func`]: the
//! parameter tuple `P` and the return type `R` are checked at
//! handle acquisition (via [`Func::typed`]) against the export's
//! declared component-level signature, and at call time native Rust
//! values flow across the canonical-ABI boundary without the caller
//! ever constructing a [`Val`].
//!
//! The handle's parameter tuple and return type are constrained by
//! the polyfill's own [`ComponentParameters`] / [`ComponentResult`]
//! traits — the lift/lower/typed-descriptor triple Wasmtime exposes
//! at `wasmtime::component`, named in the polyfill's surface and
//! restricted to the synchronous-baseline valtypes.
//!
//! [`Val`]: crate::Val

use core::marker::PhantomData;

use crate::component::FunctionType;
use crate::concurrency::Accessor;
use crate::error::{Error, Result, TypeMismatch, TypeMismatchPosition, TypeRendering};
use crate::internal::{FuncInternal, TypedFuncInternal};
use crate::linker::{ComponentParameters, ComponentResult};
use crate::store::Store;
use crate::value::Val;

use super::func::Func;

/// A statically-typed handle for invoking one component export.
///
/// `TypedFunc<P, R>` is acquired from [`Func::typed`]; the conversion
/// checks that the export's declared signature matches the function
/// type derived from `P` and `R` and surfaces a structured
/// [`Error::TypeMismatch`] on mismatch. Calling the handle takes a
/// native Rust tuple and returns the native Rust return value;
/// internally the call delegates to [`Func::call`], so heap-
/// allocating valtypes drive the same `cabi_realloc` /
/// `post-return` round-trip the untyped path observes.
///
/// [`Func::typed`]: super::Func::typed
/// [`Func::call`]: super::Func::call
pub struct TypedFunc<P, R> {
    inner: Func,
    _phantom: PhantomData<fn(P) -> R>,
}

impl<P, R> core::fmt::Debug for TypedFunc<P, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TypedFunc")
            .field("name", &self.inner.name())
            .field("signature", &self.inner.signature())
            .finish()
    }
}

impl<P, R> TypedFuncInternal<P, R> for TypedFunc<P, R> {
    fn from_checked(inner: Func) -> TypedFunc<P, R> {
        TypedFunc {
            inner,
            _phantom: PhantomData,
        }
    }
}

impl<P, R> TypedFunc<P, R>
where
    P: ComponentParameters,
    R: ComponentResult,
{
    /// Invoke the export with native Rust values.
    ///
    /// `args` is the parameter tuple `P`; the return is the
    /// native Rust value `R` the export produces. Compound
    /// argument and return shapes drive the same canonical-ABI
    /// round-trip — `cabi_realloc` for heap-allocating values and
    /// `post-return` after the result is observed — that the
    /// untyped [`Func::call`] path observes.
    ///
    /// `T` is the host-data type of the [`Store`] the export's
    /// owning [`Instance`] was created in.
    ///
    /// [`Func::call`]: super::Func::call
    /// [`Store`]: crate::Store
    /// [`Instance`]: super::Instance
    pub async fn call<T: 'static>(&self, store: &mut Store<T>, args: P) -> Result<R> {
        let lowered = args.into_vals();
        let results = self.inner.call(store, &lowered).await?;
        typed_result(&results)
    }

    /// Invoke the export from inside a poll of the store, with
    /// native Rust values.
    ///
    /// This is the typed counterpart to [`Func::call_concurrent`]:
    /// `args` is the parameter tuple `P` and the return is the
    /// native Rust value `R`, and everything else is that entry's.
    /// The call reaches the store through the accessor a
    /// `run_concurrent` closure or a host `async` function's body
    /// holds, creates the export's task, and queues its start behind
    /// the entry gate of the export's instance. The returned future
    /// resolves when the task's result is set. It is spawn-like:
    /// dropping it cancels nothing, the task progresses only while a
    /// driver runs turns, and a store that goes idle with the task
    /// unresolved leaves the future pending rather than failing it.
    /// It registers the waker it is polled with and is woken when
    /// the task resolves or fails, so several such calls can be
    /// awaited together through a waker-gated combinator.
    ///
    /// `T` is the host-data type of the [`Store`] the export's
    /// owning [`Instance`] was created in.
    ///
    /// [`Func::call_concurrent`]: super::Func::call_concurrent
    /// [`Store`]: crate::Store
    /// [`Instance`]: super::Instance
    pub async fn call_concurrent<T: 'static>(&self, accessor: &Accessor<T>, args: P) -> Result<R> {
        let lowered = args.into_vals();
        let results = self.inner.call_concurrent(accessor, &lowered).await?;
        typed_result(&results)
    }
}

/// The native Rust value an export's returned values stand for. A
/// component function declares at most one result, so anything else
/// is the polyfill disagreeing with itself.
fn typed_result<R: ComponentResult>(results: &[Val]) -> Result<R> {
    match results.len() {
        0 => R::from_val(None),
        1 => R::from_val(Some(&results[0])),
        n => Err(Error::Internal {
            message: format!(
                "typed export call observed {n} return values; an export admits at most one"
            ),
        }),
    }
}

impl Func {
    /// Convert this untyped function handle into a typed one whose
    /// parameter tuple `P` and return type `R` are checked against
    /// the export's declared component-level signature.
    ///
    /// Returns [`Error::TypeMismatch`] when `P` and `R` derive a
    /// component signature that does not equal the export's
    /// declared signature; the [`TypeMismatch`]'s `position` is
    /// [`TypeMismatchPosition::TypedConversion`] and its
    /// `expected` / `actual` renderings carry the export-side and
    /// requested-from-Rust signatures, respectively.
    pub fn typed<P, R>(self) -> Result<TypedFunc<P, R>>
    where
        P: ComponentParameters,
        R: ComponentResult,
    {
        // The requested type is never `async`: a host's Rust tuple
        // says nothing about the effect, and `signatures_compatible`
        // ignores the flag, so a typed handle to a callback export
        // is acquired with the same call a synchronous export takes.
        let requested = FunctionType {
            parameters: P::parameter_types(),
            result: R::result_type(),
            async_: false,
        };
        if !signatures_compatible(self.signature(), &requested) {
            return Err(Error::from(TypeMismatch {
                position: TypeMismatchPosition::TypedConversion {
                    export: self.name().to_owned(),
                },
                expected: TypeRendering::Function(self.signature().clone()),
                actual: TypeRendering::Function(requested),
            }));
        }
        Ok(TypedFunc::from_checked(self))
    }
}

/// Two function types are compatible for the typed-conversion entry
/// point when their parameter and result `ValueType`s match
/// positionally and structurally. The `async` flag is ignored too: a
/// callback export and a synchronous export of the same shape take
/// the same arguments and produce the same result, so a host
/// acquires a typed handle to either with the same code.
/// Parameter *names* are ignored as well: the
/// component-side declares the names of the parameters in WIT, but
/// the host's typed-conversion call site supplies a Rust tuple
/// without names — the polyfill synthesises `arg0, arg1, …` for the
/// requested signature. Comparing names directly would force every
/// caller to know the WIT parameter names, which is a surface a
/// host-binding code generator covers; the polyfill checks the
/// types instead.
fn signatures_compatible(declared: &FunctionType, requested: &FunctionType) -> bool {
    if declared.parameters.len() != requested.parameters.len() {
        return false;
    }
    for (left, right) in declared.parameters.iter().zip(requested.parameters.iter()) {
        if left.ty != right.ty {
            return false;
        }
    }
    declared.result == requested.result
}
