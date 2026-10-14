// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

use crate::abi::call_values::{check_store, lift_result_value, lower_arguments};
use crate::abi::context::BoundaryContext;
use crate::abi::signature::Signature;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreId;
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

    /// Refuse arguments that hold a host value of a store other than
    /// `store`, with the wrong-store cause, before the call creates
    /// anything in the store. Only an argument whose type in
    /// `signature` holds an error context can. A lower would refuse the
    /// value too, but only once the call's task runs, where a failure is
    /// a trap that poisons the store.
    fn check_store(&self, signature: &Signature, store: StoreId) -> Result<()>;

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

    /// Only the arguments whose parameter type holds an error context
    /// are walked.
    fn check_store(&self, signature: &Signature, store: StoreId) -> Result<()> {
        signature
            .error_context_params()
            .iter()
            .filter_map(|&index| self.get(index))
            .try_for_each(|arg| check_store(arg, store))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::FunctionParameter;
    use crate::concurrency::{ErrorContextAny, ErrorContextId};
    use crate::error::InstantiationError;
    use crate::internal::{ErrorContextAnyInternal, StoreIdInternal};
    use crate::types::{ListType, PrimitiveType, ValueType};

    fn parameter(name: &str, ty: ValueType) -> FunctionParameter {
        FunctionParameter {
            name: name.to_owned(),
            ty,
        }
    }

    #[wcmp_macros::test]
    fn it_walks_only_the_arguments_whose_type_holds_an_error_context() {
        let store = StoreId::fresh();
        let foreign = Val::ErrorContext(ErrorContextAny::new(
            StoreId::fresh(),
            ErrorContextId::new(0, 0),
        ));
        let local = Val::ErrorContext(ErrorContextAny::new(store, ErrorContextId::new(0, 0)));
        let signature = Signature::new(FunctionType {
            parameters: vec![
                parameter(
                    "bytes",
                    ValueType::List(ListType::new(ValueType::Primitive(PrimitiveType::U8))),
                ),
                parameter("error", ValueType::ErrorContext),
            ],
            result: None,
            async_: false,
        });
        assert_eq!(signature.error_context_params(), &[1]);

        // The first argument does not match its `list<u8>` type, and the
        // lower would refuse it; the pre-check does not look at it, so the
        // foreign error context inside it passes.
        let unwalked = vec![Val::List(vec![foreign.clone()].into()), local.clone()];
        unwalked
            .check_store(&signature, store)
            .expect("the list<u8> argument is not walked");

        let walked = vec![Val::List(Vec::new().into()), foreign];
        let error = walked
            .check_store(&signature, store)
            .expect_err("the error-context argument is walked");
        assert!(
            matches!(&error, Error::Instantiation(inner) if matches!(**inner, InstantiationError::WrongStore)),
            "{error:?}"
        );
    }
}
