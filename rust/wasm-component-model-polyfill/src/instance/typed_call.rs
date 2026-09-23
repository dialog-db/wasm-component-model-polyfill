//! The values of one typed export call.

use core::marker::PhantomData;

use wasm_runtime_layer::Val as RuntimeVal;

use crate::abi::context::BoundaryContext;
use crate::abi::signature::Signature;
use crate::component::FunctionType;
use crate::error::Result;
use crate::linker::{ComponentParameters, ComponentResult};
use crate::value::Val;

use super::call_values::CallValues;

/// The Rust argument tuple of one typed export call, and the Rust
/// type its result comes back as.
///
/// The arguments are lowered straight into the guest and the result
/// lifted straight out of it, through the hidden direct methods of
/// the typed value traits: a string or a vector of numbers crosses as
/// one block of bytes, and no `Val` is built for it.
pub struct TypedCall<P, R> {
    arguments: P,
    _result: PhantomData<fn() -> R>,
}

impl<P, R> TypedCall<P, R> {
    /// The call that carries `arguments`.
    pub fn new(arguments: P) -> Self {
        Self {
            arguments,
            _result: PhantomData,
        }
    }
}

impl<P: ComponentParameters, R: ComponentResult> CallValues for TypedCall<P, R> {
    type Output = R;

    /// The handle checked its whole signature when it was acquired,
    /// the parameter count with it.
    fn check_arity(&self, _signature: &FunctionType) -> Result<()> {
        Ok(())
    }

    fn lower<T: 'static>(
        self,
        cx: &mut BoundaryContext<'_, T>,
        signature: &Signature,
    ) -> Result<Vec<RuntimeVal>> {
        self.arguments.lower_arguments(cx, signature)
    }

    fn lift<T: 'static>(
        cx: &mut BoundaryContext<'_, T>,
        core_results: &[RuntimeVal],
        signature: &FunctionType,
    ) -> Result<R> {
        R::lift_result(cx, core_results, signature)
    }

    /// A typed call's task records no value: building one would cost
    /// the host a `Val` per element of the result, which is what the
    /// typed call is for avoiding, and the caller takes the result
    /// from the call.
    fn resolution(_output: &R) -> Option<Val> {
        None
    }

    fn from_resolution(value: Option<Val>) -> Result<R> {
        R::from_val(value.as_ref())
    }
}
