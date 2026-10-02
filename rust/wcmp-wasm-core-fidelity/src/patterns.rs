// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The values of a script: its arguments, and the patterns its results
//! must match, read through what the host sees of a value.

use wast::core::{
    AbstractHeapType, HeapType as WastHeapType, NanPattern, V128Pattern, WastRetCore,
};
use wast::token::{F32, F64};
use wcmp_wasm_core::{AsContext, HeapType, TrapKind, Val};

/// The heap type of the runtime layer that `heap` names, or `None` for a
/// concrete or a shared heap type, whose hierarchy the runner cannot tell.
pub fn heap_type(heap: &WastHeapType<'_>) -> Option<HeapType> {
    let WastHeapType::Abstract { shared: false, ty } = heap else {
        return None;
    };
    Some(match ty {
        AbstractHeapType::Func => HeapType::Func,
        AbstractHeapType::Extern => HeapType::Extern,
        AbstractHeapType::Exn => HeapType::Exn,
        AbstractHeapType::Cont => HeapType::Cont,
        AbstractHeapType::Any => HeapType::Any,
        AbstractHeapType::Eq => HeapType::Eq,
        AbstractHeapType::Struct => HeapType::Struct,
        AbstractHeapType::Array => HeapType::Array,
        AbstractHeapType::I31 => HeapType::I31,
        AbstractHeapType::NoFunc => HeapType::NoFunc,
        AbstractHeapType::NoExtern => HeapType::NoExtern,
        AbstractHeapType::None => HeapType::None,
        AbstractHeapType::NoExn => HeapType::NoExn,
        AbstractHeapType::NoCont => HeapType::NoCont,
    })
}

/// Whether `actual`, a value of `store`, matches the pattern `expected`.
pub fn matches(store: impl AsContext, expected: &WastRetCore<'_>, actual: &Val) -> bool {
    let store = store.as_context();
    match (expected, actual) {
        (WastRetCore::I32(expected), Val::I32(actual)) => expected == actual,
        (WastRetCore::I64(expected), Val::I64(actual)) => expected == actual,
        (WastRetCore::F32(expected), Val::F32(actual)) => f32_matches(expected, *actual),
        (WastRetCore::F64(expected), Val::F64(actual)) => f64_matches(expected, *actual),
        (WastRetCore::V128(expected), Val::V128(actual)) => v128_matches(expected, *actual),
        (WastRetCore::RefNull(heap), actual) => match heap.as_ref().and_then(heap_type) {
            Some(heap) => null_of(heap, actual),
            None => actual.is_null(),
        },
        (WastRetCore::RefExtern(None), Val::ExternRef(actual)) => actual.is_some(),
        (WastRetCore::RefExtern(Some(expected)), Val::ExternRef(Some(actual))) => actual
            .data(&store)
            .ok()
            .and_then(|data| data.downcast_ref::<u32>())
            .is_some_and(|actual| actual == expected),
        (WastRetCore::RefFunc(_), Val::FuncRef(actual)) => actual.is_some(),
        // A host object inside the internal hierarchy, and every reference
        // to `any` or `eq`: the host sees only that the reference is there.
        (
            WastRetCore::RefHost(_) | WastRetCore::RefAny | WastRetCore::RefEq,
            Val::AnyRef(actual),
        ) => actual.is_some(),
        // A GC object is opaque to the host, which sees only that the
        // reference is there and is not an `i31ref`.
        (WastRetCore::RefStruct | WastRetCore::RefArray, Val::AnyRef(Some(actual))) => {
            matches!(actual.as_i31(&store), Ok(None))
        }
        (WastRetCore::RefI31 | WastRetCore::RefI31Shared, Val::AnyRef(Some(actual))) => {
            matches!(actual.as_i31(&store), Ok(Some(_)))
        }
        (WastRetCore::Either(alternatives), actual) => alternatives
            .iter()
            .any(|expected| matches(&store, expected, actual)),
        _ => false,
    }
}

/// Whether `actual` is the null of the hierarchy of `heap`.
fn null_of(heap: HeapType, actual: &Val) -> bool {
    matches!(
        (Val::null(heap), actual),
        (Val::FuncRef(None), Val::FuncRef(None))
            | (Val::ExternRef(None), Val::ExternRef(None))
            | (Val::AnyRef(None), Val::AnyRef(None))
            | (Val::ExnRef(None), Val::ExnRef(None))
            | (Val::ContRef(None), Val::ContRef(None))
    )
}

/// The bits of a float's exponent, and of the quiet bit of its NaN, for a
/// float of 32 bits.
const F32_QUIET_NAN: u32 = 0x7fc0_0000;
/// The bits of a float's sign.
const F32_SIGN: u32 = 0x8000_0000;
/// The bits of a float's exponent, and of the quiet bit of its NaN, for a
/// float of 64 bits.
const F64_QUIET_NAN: u64 = 0x7ff8_0000_0000_0000;
/// The bits of a float's sign.
const F64_SIGN: u64 = 0x8000_0000_0000_0000;

/// Whether the bits `actual` match `expected`: the same bits, a canonical
/// NaN of either sign, or an arithmetic NaN, whose quiet bit is set.
fn f32_matches(expected: &NanPattern<F32>, actual: u32) -> bool {
    match expected {
        NanPattern::Value(expected) => expected.bits == actual,
        NanPattern::CanonicalNan => actual & !F32_SIGN == F32_QUIET_NAN,
        NanPattern::ArithmeticNan => actual & F32_QUIET_NAN == F32_QUIET_NAN,
    }
}

/// Whether the bits `actual` match `expected`, as [`f32_matches`] does.
fn f64_matches(expected: &NanPattern<F64>, actual: u64) -> bool {
    match expected {
        NanPattern::Value(expected) => expected.bits == actual,
        NanPattern::CanonicalNan => actual & !F64_SIGN == F64_QUIET_NAN,
        NanPattern::ArithmeticNan => actual & F64_QUIET_NAN == F64_QUIET_NAN,
    }
}

/// Whether the vector `actual` matches `expected`, lane by lane.
fn v128_matches(expected: &V128Pattern, actual: u128) -> bool {
    let bytes = actual.to_le_bytes();
    let lanes = |width: usize| bytes.chunks_exact(width);
    match expected {
        V128Pattern::I8x16(expected) => expected
            .iter()
            .zip(lanes(1))
            .all(|(expected, lane)| expected.to_le_bytes() == lane),
        V128Pattern::I16x8(expected) => expected
            .iter()
            .zip(lanes(2))
            .all(|(expected, lane)| expected.to_le_bytes() == lane),
        V128Pattern::I32x4(expected) => expected
            .iter()
            .zip(lanes(4))
            .all(|(expected, lane)| expected.to_le_bytes() == lane),
        V128Pattern::I64x2(expected) => expected
            .iter()
            .zip(lanes(8))
            .all(|(expected, lane)| expected.to_le_bytes() == lane),
        V128Pattern::F32x4(expected) => expected.iter().zip(lanes(4)).all(|(expected, lane)| {
            let lane = u32::from_le_bytes(lane.try_into().expect("a lane of four bytes"));
            f32_matches(expected, lane)
        }),
        V128Pattern::F64x2(expected) => expected.iter().zip(lanes(8)).all(|(expected, lane)| {
            let lane = u64::from_le_bytes(lane.try_into().expect("a lane of eight bytes"));
            f64_matches(expected, lane)
        }),
    }
}

/// Whether the trap `kind` is the trap that the specification's `message`
/// names. Where the message names one core trap, the kind must be that
/// trap's. Any other message holds for any trap that is not a host error.
pub fn trap_matches(message: &str, kind: &TrapKind) -> bool {
    let expected: fn(&TrapKind) -> bool = match message {
        m if m.starts_with("unreachable") => |k| matches!(k, TrapKind::UnreachableCodeReached),
        m if m.starts_with("out of bounds memory access") => {
            |k| matches!(k, TrapKind::MemoryOutOfBounds)
        }
        m if m.starts_with("out of bounds table access") || m.starts_with("undefined element") => {
            |k| matches!(k, TrapKind::TableOutOfBounds)
        }
        m if m.starts_with("uninitialized element") => {
            |k| matches!(k, TrapKind::IndirectCallToNull)
        }
        m if m.starts_with("null") && m.ends_with("reference") => {
            |k| matches!(k, TrapKind::NullReference)
        }
        m if m.starts_with("integer divide by zero") => {
            |k| matches!(k, TrapKind::IntegerDivisionByZero)
        }
        m if m.starts_with("integer overflow") => |k| matches!(k, TrapKind::IntegerOverflow),
        m if m.starts_with("invalid conversion to integer") => {
            |k| matches!(k, TrapKind::BadConversionToInteger)
        }
        m if m.starts_with("indirect call type mismatch") => {
            |k| matches!(k, TrapKind::BadSignature)
        }
        m if m.starts_with("out of bounds array access") => {
            |k| matches!(k, TrapKind::ArrayOutOfBounds)
        }
        m if m.starts_with("cast failure") => |k| matches!(k, TrapKind::CastFailure),
        m if m.starts_with("unaligned atomic") => |k| matches!(k, TrapKind::HeapMisaligned),
        _ => |k| !matches!(k, TrapKind::Host(_)),
    };
    expected(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_matches_a_nan_by_its_class() {
        let canonical = NanPattern::<F32>::CanonicalNan;
        let arithmetic = NanPattern::<F32>::ArithmeticNan;
        assert!(f32_matches(&canonical, 0x7fc0_0000));
        assert!(f32_matches(&canonical, 0xffc0_0000));
        assert!(!f32_matches(&canonical, 0x7fc0_0001));
        assert!(f32_matches(&arithmetic, 0x7fc0_0001));
        assert!(!f32_matches(&arithmetic, 0x7f80_0001));
        assert!(!f32_matches(&arithmetic, 0x3f80_0000));
        assert!(f64_matches(
            &NanPattern::<F64>::CanonicalNan,
            0xfff8_0000_0000_0000
        ));
        assert!(!f64_matches(
            &NanPattern::<F64>::ArithmeticNan,
            0x7ff0_0000_0000_0001
        ));
    }

    #[wcmp_macros::test]
    fn it_matches_a_vector_lane_by_lane() {
        let vector =
            u128::from_le_bytes([1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        assert!(v128_matches(&V128Pattern::I32x4([1, 2, 3, -1]), vector));
        assert!(!v128_matches(&V128Pattern::I32x4([1, 2, 3, 4]), vector));
        assert!(v128_matches(
            &V128Pattern::I16x8([1, 0, 2, 0, 3, 0, -1, -1]),
            vector
        ));
    }

    #[wcmp_macros::test]
    fn it_holds_a_trap_to_the_kind_its_message_names() {
        assert!(trap_matches(
            "integer divide by zero",
            &TrapKind::IntegerDivisionByZero
        ));
        assert!(!trap_matches(
            "integer divide by zero",
            &TrapKind::IntegerOverflow
        ));
        assert!(trap_matches(
            "out of bounds memory access",
            &TrapKind::MemoryOutOfBounds
        ));
        assert!(!trap_matches(
            "out of bounds memory access",
            &TrapKind::Host(anyhow_error())
        ));
    }

    fn anyhow_error() -> anyhow::Error {
        anyhow::Error::msg("the host failed")
    }
}
