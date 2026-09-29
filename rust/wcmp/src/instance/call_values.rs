//! What one call into an export carries in and brings back out.
//!
//! An export call lowers its arguments, runs the core function, and
//! lifts its result, and every step between — the task, the entry
//! gate, the scheduler's item, the driver — is the same whatever the
//! values are. [`CallValues`] is the part that is not: how the
//! arguments reach the guest and in what shape the result comes back.
//! The untyped call carries a slice of [`Val`]s and brings back a
//! boxed slice of them. The typed call carries a Rust tuple and brings
//! back a Rust value, and never holds a `Val` for either unless the
//! export resolves through `task.return`.

use crate::abi::call_values::{lift_result_value, lower_arguments};
use crate::abi::context::BoundaryContext;
use crate::abi::signature::Signature;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::runtime_layer::Val as RuntimeVal;
use crate::value::Val;

/// The arguments of one export call and the shape its result comes
/// back in.
///
/// The value is moved into the scheduler's item that runs the call,
/// which outlives the call's future, so it owns everything it lowers.
pub trait CallValues: Send + 'static {
    /// What the call returns to its caller.
    type Output: Send + 'static;

    /// Refuse arguments that are not the export's parameter count,
    /// before the call creates anything in the store.
    fn check_arity(&self, signature: &FunctionType) -> Result<()>;

    /// Lower the arguments as the parameters of `signature` and answer
    /// the core arguments of the call. The signature carries the
    /// layout of a spilled parameter tuple, computed once.
    fn lower<T: 'static>(
        self,
        cx: &mut BoundaryContext<'_, T>,
        signature: &Signature,
    ) -> Result<Vec<RuntimeVal>>;

    /// Lift the result of `signature` out of the core results the
    /// export returned. The export's `post-return` runs after this,
    /// so nothing the lift reads is freed under it.
    fn lift<T: 'static>(
        cx: &mut BoundaryContext<'_, T>,
        core_results: &[RuntimeVal],
        signature: &FunctionType,
    ) -> Result<Self::Output>;

    /// The value the call's task resolves with. The caller takes the
    /// call's result from the call rather than from the task, so this
    /// is only the task's own record of it.
    fn resolution(output: &Self::Output) -> Option<Val>;

    /// The call's result from the value an export lifted `async`
    /// resolved its task with through `task.return`, which is a
    /// [`Val`] whatever the call carries.
    fn from_resolution(value: Option<Val>) -> Result<Self::Output>;
}

/// The untyped call: one [`Val`] per argument in, and the lifted
/// result back as a boxed slice of at most one.
impl CallValues for Vec<Val> {
    type Output = Box<[Val]>;

    fn check_arity(&self, signature: &FunctionType) -> Result<()> {
        if self.len() == signature.parameters.len() {
            return Ok(());
        }
        Err(Error::from(AbiError {
            position: AbiPosition::Argument(0),
            valtype: None,
            cause: AbiCause::InvalidEncoding {
                message: format!(
                    "expected {} arguments, got {}",
                    signature.parameters.len(),
                    self.len()
                ),
            },
        }))
    }

    fn lower<T: 'static>(
        self,
        cx: &mut BoundaryContext<'_, T>,
        signature: &Signature,
    ) -> Result<Vec<RuntimeVal>> {
        lower_arguments(cx, signature, &self)
    }

    fn lift<T: 'static>(
        cx: &mut BoundaryContext<'_, T>,
        core_results: &[RuntimeVal],
        signature: &FunctionType,
    ) -> Result<Box<[Val]>> {
        Ok(lift_result_value(cx, core_results, signature)?
            .into_iter()
            .collect())
    }

    fn resolution(output: &Box<[Val]>) -> Option<Val> {
        output.first().cloned()
    }

    fn from_resolution(value: Option<Val>) -> Result<Box<[Val]>> {
        Ok(value.into_iter().collect())
    }
}
