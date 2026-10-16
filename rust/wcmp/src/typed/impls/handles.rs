// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The values that are an index into a guest's handle table: the
//! readable end of a stream or a future, as Wasmtime's
//! `StreamReader<T>` and `FutureReader<T>`, and an error context.
//!
//! Each crosses as the `u32` index of its entry. Lowering a reader
//! enters its end in the guest's table, and lifting one takes the
//! guest's entry away and hands the host the end. A lift checks the
//! entry before it takes it, so a refused lift leaves the guest's
//! entry where it was. Lifting an error context leaves the guest's
//! handle where it was and marks the record host-held, and lowering
//! one gives the guest a handle of its own.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::{
    lift_end_id_for_host, lift_error_context_id, lower_error_context, lower_host_end,
};
use crate::concurrency::{
    EndId, ErrorContext, ErrorContextAny, FutureAny, FutureReader, StreamAny, StreamReader,
};
use crate::error::{AbiPosition, Error, InstantiationError, Result, TypePathStep};
use crate::internal::{
    ErrorContextAnyInternal, ErrorContextInternal, FutureAnyInternal, FutureReaderInternal,
    LiftContextInternal, LowerContextInternal, StreamAnyInternal, StreamReaderInternal,
};
use crate::runtime_layer::Val as CoreVal;
use crate::store::StoreId;
use crate::types::{ValueType, resolve_resources};

use super::super::component_type::ComponentType;
use super::super::flat::take_i32;
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

/// Read the `u32` index of a handle out of the flat values at `cursor`.
fn index_flat(
    ty: &ValueType,
    slots: &[CoreVal],
    cursor: &mut usize,
    position: AbiPosition,
) -> Result<u32> {
    Ok(take_i32(slots, cursor, ty, position)? as u32)
}

/// Read the `u32` index of a handle out of guest memory at `offset`.
fn index_memory<D: 'static>(
    cx: &mut LiftContext<'_, '_, D>,
    ty: &ValueType,
    offset: usize,
) -> Result<u32> {
    let position = cx.position();
    Ok(u32::from_le_bytes(
        cx.boundary().read_array(offset, position, ty)?,
    ))
}

/// Write the `u32` index of a handle into guest memory at `offset`.
fn write_index<D: 'static>(
    cx: &mut LowerContext<'_, '_, D>,
    ty: &ValueType,
    offset: usize,
    index: u32,
) -> Result<()> {
    let position = cx.position();
    cx.boundary()
        .write_bytes(offset, &index.to_le_bytes(), position, ty)
}

/// The readers of a stream and of a future, which differ only in the
/// kind of end and the declared type they match.
macro_rules! reader {
    ($reader:ident, $declared:ident, $step:ident, $internal:ident) => {
        impl<T: ComponentType> ComponentType for $reader<T> {
            const ABI: CanonicalAbiInfo = CanonicalAbiInfo::SCALAR4;

            fn typecheck(
                ty: &ValueType,
                types: &TypeChecker<'_>,
            ) -> core::result::Result<(), TypeCheckError> {
                match ty {
                    // A stream or a future with no payload is one of
                    // `()`, as Wasmtime's `typecheck_payload` reads it.
                    ValueType::$declared(declared) => match declared.payload() {
                        Some(payload) => T::typecheck(payload, types)
                            .map_err(|error| error.at(TypePathStep::$step)),
                        None if T::IS_RUST_UNIT_TYPE => Ok(()),
                        None => Err(TypeCheckError::mismatch::<Self>(ty)),
                    },
                    _ => Err(TypeCheckError::mismatch::<Self>(ty)),
                }
            }
        }

        impl<T: ComponentType> Lower for $reader<T> {
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                out: &mut Vec<CoreVal>,
            ) -> Result<()> {
                let position = cx.position();
                let index = lower_host_end(cx.boundary(), $internal::end(self), ty, position)?;
                out.push(CoreVal::I32(index as i32));
                Ok(())
            }

            fn lower_memory<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<()> {
                let position = cx.position();
                let index = lower_host_end(cx.boundary(), $internal::end(self), ty, position)?;
                write_index(cx, ty, offset, index)
            }
        }

        impl<T: ComponentType> Lift for $reader<T> {
            fn lift_flat<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                slots: &[CoreVal],
                cursor: &mut usize,
            ) -> Result<Self> {
                let position = cx.position();
                let index = index_flat(ty, slots, cursor, position)?;
                let end = lift_end_id_for_host(cx.boundary(), index, ty, position)?;
                Ok(<Self as $internal>::from_end(end))
            }

            fn lift_memory<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<Self> {
                let position = cx.position();
                let index = index_memory(cx, ty, offset)?;
                let end = lift_end_id_for_host(cx.boundary(), index, ty, position)?;
                Ok(<Self as $internal>::from_end(end))
            }
        }
    };
}

reader!(StreamReader, Stream, StreamPayload, StreamReaderInternal);
reader!(FutureReader, Future, FuturePayload, FutureReaderInternal);

impl ComponentType for ErrorContext {
    const ABI: CanonicalAbiInfo = CanonicalAbiInfo::SCALAR4;

    fn typecheck(ty: &ValueType, _: &TypeChecker<'_>) -> core::result::Result<(), TypeCheckError> {
        match ty {
            ValueType::ErrorContext => Ok(()),
            _ => Err(TypeCheckError::mismatch::<Self>(ty)),
        }
    }
}

impl Lower for ErrorContext {
    fn lower_flat<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        out: &mut Vec<CoreVal>,
    ) -> Result<()> {
        let position = cx.position();
        let context = ErrorContextAny::new(self.store_id(), self.context());
        let index = lower_error_context(cx.boundary(), &context, ty, position)?;
        out.push(CoreVal::I32(index as i32));
        Ok(())
    }

    fn lower_memory<D: 'static>(
        &self,
        cx: &mut LowerContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<()> {
        let position = cx.position();
        let context = ErrorContextAny::new(self.store_id(), self.context());
        let index = lower_error_context(cx.boundary(), &context, ty, position)?;
        write_index(cx, ty, offset, index)
    }

    fn check_store(&self, store: StoreId) -> Result<()> {
        if self.store_id() == store {
            Ok(())
        } else {
            Err(Error::from(InstantiationError::WrongStore))
        }
    }
}

impl Lift for ErrorContext {
    fn lift_flat<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        slots: &[CoreVal],
        cursor: &mut usize,
    ) -> Result<Self> {
        let position = cx.position();
        let index = index_flat(ty, slots, cursor, position)?;
        let boundary = cx.boundary();
        let context = lift_error_context_id(boundary, index, ty, position)?;
        Ok(ErrorContext::new(boundary.store_id(), context))
    }

    fn lift_memory<D: 'static>(
        cx: &mut LiftContext<'_, '_, D>,
        ty: &ValueType,
        offset: usize,
    ) -> Result<Self> {
        let position = cx.position();
        let index = index_memory(cx, ty, offset)?;
        let boundary = cx.boundary();
        let context = lift_error_context_id(boundary, index, ty, position)?;
        Ok(ErrorContext::new(boundary.store_id(), context))
    }
}

/// The untyped readers of a stream and of a future, as Wasmtime's
/// `StreamAny` and `FutureAny` cross: their type check accepts any
/// `stream` or `future`, whatever its payload, and the lowering checks
/// the payload the end's record carries against the declared one.
macro_rules! any_reader {
    ($any:ident, $declared:ident, $internal:ident) => {
        impl ComponentType for $any {
            const ABI: CanonicalAbiInfo = CanonicalAbiInfo::SCALAR4;

            fn typecheck(
                ty: &ValueType,
                _: &TypeChecker<'_>,
            ) -> core::result::Result<(), TypeCheckError> {
                match ty {
                    ValueType::$declared(_) => Ok(()),
                    _ => Err(TypeCheckError::mismatch::<Self>(ty)),
                }
            }
        }

        impl Lower for $any {
            fn lower_flat<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                out: &mut Vec<CoreVal>,
            ) -> Result<()> {
                let position = cx.position();
                let index = lower_host_end(cx.boundary(), $internal::end(self), ty, position)?;
                out.push(CoreVal::I32(index as i32));
                Ok(())
            }

            fn lower_memory<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<()> {
                let position = cx.position();
                let index = lower_host_end(cx.boundary(), $internal::end(self), ty, position)?;
                write_index(cx, ty, offset, index)
            }
        }

        impl Lift for $any {
            fn lift_flat<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                slots: &[CoreVal],
                cursor: &mut usize,
            ) -> Result<Self> {
                let position = cx.position();
                let index = index_flat(ty, slots, cursor, position)?;
                lift_any(cx, ty, index)
            }

            fn lift_memory<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<Self> {
                let index = index_memory(cx, ty, offset)?;
                lift_any(cx, ty, index)
            }
        }
    };
}

any_reader!(StreamAny, Stream, StreamAnyInternal);
any_reader!(FutureAny, Future, FutureAnyInternal);

/// Lift the readable end at `index` toward the host as an untyped
/// reader of `ty`, whose payload names its resources with the
/// identities the instantiation gave them, as the end's record does.
fn lift_any<D: 'static, A: AnyReader>(
    cx: &mut LiftContext<'_, '_, D>,
    ty: &ValueType,
    index: u32,
) -> Result<A> {
    let position = cx.position();
    let boundary = cx.boundary();
    let end = lift_end_id_for_host(boundary, index, ty, position)?;
    let payload = match ty {
        ValueType::Stream(stream) => stream.payload(),
        ValueType::Future(future) => future.payload(),
        _ => None,
    };
    let resolve = |declared| boundary.instance().resource_identity(declared);
    let payload = payload.map(|payload| resolve_resources(payload, &resolve));
    Ok(A::from_end(end, payload))
}

/// A `StreamAny` or a `FutureAny`, as [`lift_any`] makes one.
trait AnyReader {
    fn from_end(end: EndId, payload: Option<ValueType>) -> Self;
}

impl AnyReader for StreamAny {
    fn from_end(end: EndId, payload: Option<ValueType>) -> Self {
        <StreamAny as StreamAnyInternal>::new(end, payload)
    }
}

impl AnyReader for FutureAny {
    fn from_end(end: EndId, payload: Option<ValueType>) -> Self {
        <FutureAny as FutureAnyInternal>::new(end, payload)
    }
}
