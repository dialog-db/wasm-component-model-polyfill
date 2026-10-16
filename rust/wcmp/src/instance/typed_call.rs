// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The values of one typed export call.

use core::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::abi::context::BoundaryContext;
use crate::abi::signature::Signature;
use crate::component::FunctionType;
use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreId;
use crate::typed::{Lift, Lower, lift_results, lift_returned, lower_parameters};
use crate::value::Val;

use super::call_values::{CallValues, ReturnLift};

/// The Rust parameter list of one typed export call, and the Rust
/// result list it comes back as.
///
/// The parameters are lowered straight into the guest and the results
/// lifted straight out of it, through the hidden methods of the typed
/// value traits. No `Val` is built for either, at any depth, and an
/// export lifted `async` hands its result over through `task.return`,
/// which lifts it the same way.
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

/// Where the result of a typed call into an export lifted `async`
/// waits for the call: `task.return` leaves it here as it resolves the
/// task.
pub type ReturnedSlot<R> = Arc<Mutex<Option<R>>>;

impl<P, R> CallValues for TypedCall<P, R>
where
    P: Lower + 'static,
    R: Lift + 'static,
{
    type Output = R;
    type Returned = ReturnedSlot<R>;

    /// The handle checked its whole signature when it was acquired,
    /// the parameter count with it.
    fn check_arity(&self, _signature: &FunctionType) -> Result<()> {
        Ok(())
    }

    fn check_store(&self, signature: &Signature, store: StoreId) -> Result<()> {
        if !signature.params_hold_error_context() {
            return Ok(());
        }
        self.arguments.check_store(store)
    }

    fn lower<T: 'static>(
        self,
        cx: &mut BoundaryContext<'_, T>,
        signature: &Signature,
    ) -> Result<Vec<RuntimeVal>> {
        lower_parameters(cx, signature, &self.arguments)
    }

    fn lift<T: 'static>(
        cx: &mut BoundaryContext<'_, T>,
        core_results: &[RuntimeVal],
        signature: &FunctionType,
    ) -> Result<R> {
        lift_results(cx, core_results, signature)
    }

    /// A typed call's task records no value: building one would cost
    /// the host a `Val` per element of the result, which is what the
    /// typed call is for avoiding, and the caller takes the result
    /// from the call.
    fn resolution(_output: &R) -> Option<Val> {
        None
    }

    fn prepare_return<T: 'static>(
        signature: &Arc<Signature>,
    ) -> (ReturnedSlot<R>, Option<ReturnLift<T>>) {
        let slot: ReturnedSlot<R> = Arc::new(Mutex::new(None));
        let kept = slot.clone();
        let signature = Arc::clone(signature);
        let lift: ReturnLift<T> = Box::new(move |cx, args, spilled| {
            let lifted = lift_returned::<_, R>(cx, args, signature.ty(), spilled)?;
            *kept
                .lock()
                .map_err(|_| Error::internal("a typed call's result slot is poisoned"))? =
                Some(lifted);
            Ok(())
        });
        (slot, Some(lift))
    }

    fn from_resolution(_value: Option<Val>, returned: &ReturnedSlot<R>) -> Result<R> {
        returned
            .lock()
            .map_err(|_| Error::internal("a typed call's result slot is poisoned"))?
            .take()
            .ok_or_else(|| {
                Error::internal("a typed call's task resolved without its result lift running")
            })
    }
}
