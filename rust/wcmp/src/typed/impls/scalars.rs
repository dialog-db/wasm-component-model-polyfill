// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The scalars: `bool`, the integers, the floats, and `char`.
//!
//! A scalar crosses as one flat value, or as its little-endian bytes in
//! memory. A list of scalars crosses as the list's bytes: one write
//! into the guest on the way in, and one read on the way out.

use std::borrow::Cow;

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::abi::layout::FlatType;
use crate::abi::{canonical_f32, canonical_f64, gate_list, lower_list_bytes};
use crate::error::Result;
use crate::internal::{LiftContextInternal, LowerContextInternal};
use crate::runtime_layer::Val as CoreVal;
use crate::types::{PrimitiveType, ValueType};

use super::super::component_type::ComponentType;
use super::super::flat::{declared_list, declared_mismatch, invalid_encoding, take};
use super::super::lift::Lift;
use super::super::lift_context::LiftContext;
use super::super::lower::Lower;
use super::super::lower_context::LowerContext;
use super::super::type_check_error::TypeCheckError;
use super::super::type_checker::TypeChecker;

/// One scalar: its Rust type, the primitive it stands for, its
/// layout, its flat type, and the conversions between the Rust value
/// and its flat value and bytes.
macro_rules! scalar {
    (@list $rust:ty, $bytes:literal, raw, |$bv:ident| $to_bytes:expr, |$rv:ident| $from_bytes:expr) => {};
    (@list $rust:ty, $bytes:literal, , |$bv:ident| $to_bytes:expr, |$rv:ident| $from_bytes:expr) => {
        impl ListBytes for $rust {
            fn encode(items: &[Self]) -> Cow<'_, [u8]> {
                let mut bytes = Vec::with_capacity(items.len() * $bytes);
                for &$bv in items {
                    let item: [u8; $bytes] = $to_bytes;
                    bytes.extend_from_slice(&item);
                }
                Cow::Owned(bytes)
            }

            fn decode(bytes: Vec<u8>) -> Option<Vec<Self>> {
                bytes
                    .chunks_exact($bytes)
                    .map(|chunk| {
                        let mut $rv = [0u8; $bytes];
                        $rv.copy_from_slice(chunk);
                        $from_bytes
                    })
                    .collect()
            }
        }
    };
    (
        $rust:ty, $primitive:ident, $abi:ident, $flat:ident, $bytes:literal,
        to_flat: |$tv:ident| $to_flat:expr,
        from_flat: |$fv:ident| $from_flat:expr,
        to_bytes: |$bv:ident| $to_bytes:expr,
        from_bytes: |$rv:ident| $from_bytes:expr
        $(, list: $list:ident)? $(,)?
    ) => {
        scalar!(@list $rust, $bytes, $($list)?, |$bv| $to_bytes, |$rv| $from_bytes);

        impl ComponentType for $rust {
            const ABI: CanonicalAbiInfo = CanonicalAbiInfo::$abi;

            fn typecheck(ty: &ValueType, _: &TypeChecker<'_>) -> core::result::Result<(), TypeCheckError> {
                match ty {
                    ValueType::Primitive(PrimitiveType::$primitive) => Ok(()),
                    _ => Err(TypeCheckError::mismatch::<Self>(ty)),
                }
            }
        }

        impl Lower for $rust {
            fn lower_flat<D: 'static>(
                &self,
                _: &mut LowerContext<'_, '_, D>,
                _: &ValueType,
                out: &mut Vec<CoreVal>,
            ) -> Result<()> {
                let $tv = *self;
                out.push(CoreVal::$flat($to_flat));
                Ok(())
            }

            fn lower_memory<D: 'static>(
                &self,
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<()> {
                let position = cx.position();
                let $bv = *self;
                let bytes: [u8; $bytes] = $to_bytes;
                cx.boundary().write_bytes(offset, &bytes, position, ty)
            }

            fn lower_list<D: 'static>(
                items: &[Self],
                cx: &mut LowerContext<'_, '_, D>,
                ty: &ValueType,
            ) -> Result<usize> {
                let position = cx.position();
                let element = declared_list(ty).ok_or_else(|| declared_mismatch(ty, position))?;
                let bytes = <Self as ListBytes>::encode(items);
                lower_list_bytes(cx.boundary(), &bytes, element, ty, position)
            }
        }

        impl Lift for $rust {
            fn lift_flat<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                slots: &[CoreVal],
                cursor: &mut usize,
            ) -> Result<Self> {
                let position = cx.position();
                match take(slots, cursor, FlatType::$flat, ty, position)? {
                    CoreVal::$flat($fv) => $from_flat.ok_or_else(|| scalar_error(ty, position)),
                    _ => Err(declared_mismatch(ty, position)),
                }
            }

            fn lift_memory<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                offset: usize,
            ) -> Result<Self> {
                let position = cx.position();
                let $rv: [u8; $bytes] = cx.boundary().read_array(offset, position, ty)?;
                $from_bytes.ok_or_else(|| scalar_error(ty, position))
            }

            fn lift_list<D: 'static>(
                cx: &mut LiftContext<'_, '_, D>,
                ty: &ValueType,
                ptr: usize,
                len: usize,
            ) -> Result<Vec<Self>> {
                let position = cx.position();
                let element = declared_list(ty).ok_or_else(|| declared_mismatch(ty, position))?;
                let boundary = cx.boundary();
                let (byte_len, _) = gate_list(boundary, ptr, len, element, ty, position)?;
                boundary.charge_copy_budget(len, core::mem::size_of::<Self>(), position, ty)?;
                if byte_len == 0 {
                    return Ok(Vec::new());
                }
                let bytes = boundary.read_bytes(ptr, byte_len, position, ty)?;
                <Self as ListBytes>::decode(bytes).ok_or_else(|| scalar_error(element, position))
            }
        }
    };
}

/// A list of scalars as its little-endian bytes.
trait ListBytes: Sized {
    /// The bytes of `items`, one element after another.
    fn encode(items: &[Self]) -> Cow<'_, [u8]>;
    /// The elements whose bytes are `bytes`, which hold a whole number
    /// of them, or `None` when one of them is no value of the type.
    fn decode(bytes: Vec<u8>) -> Option<Vec<Self>>;
}

/// A list of bytes is its own bytes, in both directions, so it crosses
/// in the one copy the write or the read makes.
impl ListBytes for u8 {
    fn encode(items: &[Self]) -> Cow<'_, [u8]> {
        Cow::Borrowed(items)
    }

    fn decode(bytes: Vec<u8>) -> Option<Vec<Self>> {
        Some(bytes)
    }
}

/// The failure of a scalar whose bits are no value of its type: a
/// `char` that is not a Unicode scalar value, the only scalar with
/// bits that are not a value.
fn scalar_error(ty: &ValueType, position: crate::error::AbiPosition) -> crate::Error {
    invalid_encoding(ty, position, "char value is not a valid Unicode scalar")
}

scalar!(bool, Bool, SCALAR1, I32, 1,
    to_flat: |v| i32::from(v),
    from_flat: |v| Some(v != 0),
    to_bytes: |v| [u8::from(v)],
    from_bytes: |b| Some(b[0] != 0),
);
scalar!(i8, S8, SCALAR1, I32, 1,
    to_flat: |v| i32::from(v),
    from_flat: |v| Some(v as i8),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(i8::from_le_bytes(b)),
);
scalar!(u8, U8, SCALAR1, I32, 1,
    to_flat: |v| i32::from(v),
    from_flat: |v| Some(v as u8),
    to_bytes: |v| [v],
    from_bytes: |b| Some(b[0]),
    list: raw,
);
scalar!(i16, S16, SCALAR2, I32, 2,
    to_flat: |v| i32::from(v),
    from_flat: |v| Some(v as i16),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(i16::from_le_bytes(b)),
);
scalar!(u16, U16, SCALAR2, I32, 2,
    to_flat: |v| i32::from(v),
    from_flat: |v| Some(v as u16),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(u16::from_le_bytes(b)),
);
scalar!(i32, S32, SCALAR4, I32, 4,
    to_flat: |v| v,
    from_flat: |v| Some(v),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(i32::from_le_bytes(b)),
);
scalar!(u32, U32, SCALAR4, I32, 4,
    to_flat: |v| v as i32,
    from_flat: |v| Some(v as u32),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(u32::from_le_bytes(b)),
);
scalar!(i64, S64, SCALAR8, I64, 8,
    to_flat: |v| v,
    from_flat: |v| Some(v),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(i64::from_le_bytes(b)),
);
scalar!(u64, U64, SCALAR8, I64, 8,
    to_flat: |v| v as i64,
    from_flat: |v| Some(v as u64),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(u64::from_le_bytes(b)),
);
// A lifted NaN is canonical, as `canonical_f32` states.
scalar!(f32, F32, SCALAR4, F32, 4,
    to_flat: |v| v.to_bits(),
    from_flat: |v| Some(canonical_f32(f32::from_bits(v))),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(canonical_f32(f32::from_le_bytes(b))),
);
scalar!(f64, F64, SCALAR8, F64, 8,
    to_flat: |v| v.to_bits(),
    from_flat: |v| Some(canonical_f64(f64::from_bits(v))),
    to_bytes: |v| v.to_le_bytes(),
    from_bytes: |b| Some(canonical_f64(f64::from_le_bytes(b))),
);
scalar!(char, Char, SCALAR4, I32, 4,
    to_flat: |v| v as i32,
    from_flat: |v| char::from_u32(v as u32),
    to_bytes: |v| (v as u32).to_le_bytes(),
    from_bytes: |b| char::from_u32(u32::from_le_bytes(b)),
);
