//! Canonical-ABI layout: size, alignment, and flat-slot count.
//!
//! Size and alignment are computed via
//! [`wasmtime_environ::component::CanonicalAbiInfo`], the
//! spec-conformant constructors wasmtime exposes for the per-shape
//! arithmetic. The [`flat_types`] function produces the ordered list
//! of [`FlatType`] slots a value lowers to, which is the one piece
//! wasmtime-environ does not expose at function granularity (its
//! public flat-types accessor is a method on the wasmtime types
//! builder that takes an `InterfaceType`, a type the polyfill does
//! not project from `ValueType`).
//!
//! A compound type computes both once, in its constructor, and
//! carries them as its [`AbiShape`]; the functions here read that
//! shape back, so a crossing never walks a type to lay it out.
//!
//! [`AbiShape`]: crate::abi::shape::AbiShape

use std::borrow::Cow;

use wasmtime_environ::component::CanonicalAbiInfo;

pub use wasmtime_environ::component::FlatType;

use crate::component::FunctionType;
use crate::internal::CompoundTypeInternal;
use crate::types::{FlagsType, PrimitiveType, ValueType};

/// The largest number of flat core-Wasm parameter slots a call
/// passes directly. Beyond this the canonical ABI spills the whole
/// parameter tuple into linear memory and passes one `i32` pointer.
pub const MAX_FLAT_PARAMS: usize = 16;

/// The largest number of flat core-Wasm result slots a call returns
/// directly. Beyond this the canonical ABI returns the result
/// through a pointer into linear memory.
pub const MAX_FLAT_RESULTS: usize = 1;

/// The largest number of flat core-Wasm parameter slots an
/// asynchronous lower passes directly. The canonical ABI holds such
/// a lower to a lower limit than [`MAX_FLAT_PARAMS`]; beyond it the
/// whole parameter tuple spills into linear memory and one `i32`
/// pointer is passed. The matching result limit is zero, so an
/// asynchronous lower never returns a flat result.
pub const MAX_FLAT_ASYNC_PARAMS: usize = 4;

/// Round `offset` up to the next multiple of `alignment`. The
/// alignment must be a power of two.
pub fn align_to(offset: usize, alignment: usize) -> usize {
    debug_assert!(alignment.is_power_of_two());
    (offset + alignment - 1) & !(alignment - 1)
}

/// The total flat-slot count of a signature's parameter tuple, or
/// `None` when the tuple must be spilled to memory because one
/// parameter alone exceeds the flat limit.
pub fn flat_param_count(signature: &FunctionType) -> Option<usize> {
    signature
        .parameters
        .iter()
        .try_fold(0usize, |acc, p| flat_count(&p.ty).map(|n| acc + n))
}

/// Whether the parameter tuple of `signature` is passed as flat
/// slots (`false`) or spilled through one pointer (`true`).
pub fn params_spill(signature: &FunctionType) -> bool {
    !matches!(flat_param_count(signature), Some(n) if n <= MAX_FLAT_PARAMS)
}

/// Whether the result of `signature` is returned in flat slots
/// (`false`) or through a pointer into memory (`true`). A signature
/// without a result never spills.
pub fn result_spills(signature: &FunctionType) -> bool {
    match &signature.result {
        None => false,
        Some(ty) => !matches!(flat_count(ty), Some(n) if n <= MAX_FLAT_RESULTS),
    }
}

/// The memory layout of a parameter tuple spilled to linear memory:
/// the byte offset of each element, followed by the tuple's total
/// size and alignment. The layout follows the canonical ABI's record
/// rules, so the tuple is laid out exactly as `tuple<…>` of the
/// parameter types would be.
pub fn spill_layout(types: &[ValueType]) -> SpillLayout {
    let mut offsets = Vec::with_capacity(types.len());
    let mut offset = 0usize;
    let mut alignment = 1usize;
    for ty in types {
        let align = alignment_of(ty);
        alignment = alignment.max(align);
        offset = align_to(offset, align);
        offsets.push(offset);
        offset += size_of(ty);
    }
    SpillLayout {
        offsets,
        size: align_to(offset, alignment),
        alignment,
    }
}

/// The layout [`spill_layout`] computes.
#[derive(Debug)]
pub struct SpillLayout {
    /// The byte offset of each element from the start of the tuple.
    pub offsets: Vec<usize>,
    /// The total size in bytes, rounded up to the alignment.
    pub size: usize,
    /// The alignment in bytes: the largest element alignment.
    pub alignment: usize,
}

/// The number of `i32` slots a `flags` value occupies in the flat
/// representation: one per 32 flags, and zero for an empty set.
pub fn flags_chunk_count(flags: &FlagsType) -> usize {
    let n = flags.names().len();
    if n == 0 { 0 } else { n.div_ceil(32) }
}

/// The canonical-ABI byte alignment for a value type, in 32-bit
/// linear-memory mode (the only mode the polyfill exercises).
pub fn alignment_of(ty: &ValueType) -> usize {
    canonical_abi(ty).align32 as usize
}

/// The canonical-ABI byte size for a value type, in 32-bit
/// linear-memory mode.
pub fn size_of(ty: &ValueType) -> usize {
    canonical_abi(ty).size32 as usize
}

/// The number of flat core-Wasm slots a value type flattens to.
/// `None` indicates the type is too large to flatten and must be
/// passed via memory.
pub fn flat_count(ty: &ValueType) -> Option<usize> {
    canonical_abi(ty).flat_count.map(usize::from)
}

/// The byte width of the discriminant for a tagged type with `n`
/// cases (variant, option, result, enum). Per the spec: 1 byte for
/// ≤ 256 cases, 2 bytes for ≤ 65 536, 4 bytes otherwise.
pub fn discriminant_size(case_count: usize) -> usize {
    match case_count {
        0..=0x100 => 1,
        n if n <= 0x1_0000 => 2,
        _ => 4,
    }
}

/// The [`CanonicalAbiInfo`] of a polyfill [`ValueType`]. A compound
/// type computed its own when it was built, from its children's, so
/// this reads it back rather than walking the type; the leaves are
/// constants.
pub fn canonical_abi(ty: &ValueType) -> CanonicalAbiInfo {
    match ty {
        ValueType::Primitive(prim) => primitive_abi(*prim),
        ValueType::Record(record) => record.abi_shape().info().clone(),
        ValueType::Tuple(tuple) => tuple.abi_shape().info().clone(),
        ValueType::Variant(variant) => variant.abi_shape().info().clone(),
        ValueType::Option(option) => option.abi_shape().info().clone(),
        ValueType::Result(result) => result.abi_shape().info().clone(),
        ValueType::FixedLengthList(fixed) => fixed.abi_shape().info().clone(),
        ValueType::Enum(en) => CanonicalAbiInfo::enum_(en.cases().len()),
        ValueType::Flags(flags) => CanonicalAbiInfo::flags(flags.names().len()),
        ValueType::List(_) | ValueType::Map(_) => CanonicalAbiInfo::POINTER_PAIR,
        // A stream or a future is the index of its readable end in a
        // handle table, laid out as a handle is.
        ValueType::Own(_) | ValueType::Borrow(_) | ValueType::Stream(_) | ValueType::Future(_) => {
            CanonicalAbiInfo::SCALAR4
        }
    }
}

fn primitive_abi(prim: PrimitiveType) -> CanonicalAbiInfo {
    match prim {
        PrimitiveType::Bool | PrimitiveType::S8 | PrimitiveType::U8 => CanonicalAbiInfo::SCALAR1,
        PrimitiveType::S16 | PrimitiveType::U16 => CanonicalAbiInfo::SCALAR2,
        PrimitiveType::S32 | PrimitiveType::U32 | PrimitiveType::F32 | PrimitiveType::Char => {
            CanonicalAbiInfo::SCALAR4
        }
        PrimitiveType::S64 | PrimitiveType::U64 | PrimitiveType::F64 => CanonicalAbiInfo::SCALAR8,
        PrimitiveType::String => CanonicalAbiInfo::POINTER_PAIR,
    }
}

/// The flat core-Wasm slots a value type flattens to. The length
/// matches [`flat_count`] when that returns `Some`, and the slots are
/// borrowed from the shape the type computed when it was built. A
/// type too large to flatten has no slots cached, and the list
/// returned for it is computed here in full; callers test
/// `flat_count(ty).is_some()` to decide whether to pass via flat
/// slots or via a single memory pointer.
pub fn flat_types(ty: &ValueType) -> Cow<'_, [FlatType]> {
    const I32: &[FlatType] = &[FlatType::I32];
    const I64: &[FlatType] = &[FlatType::I64];
    const F32: &[FlatType] = &[FlatType::F32];
    const F64: &[FlatType] = &[FlatType::F64];
    const PAIR: &[FlatType] = &[FlatType::I32, FlatType::I32];
    let shape = match ty {
        ValueType::Primitive(PrimitiveType::String) | ValueType::List(_) | ValueType::Map(_) => {
            return Cow::Borrowed(PAIR);
        }
        ValueType::Primitive(prim) => {
            return Cow::Borrowed(match flat_type_of_primitive(*prim) {
                FlatType::I32 => I32,
                FlatType::I64 => I64,
                FlatType::F32 => F32,
                FlatType::F64 => F64,
            });
        }
        ValueType::Enum(_)
        | ValueType::Own(_)
        | ValueType::Borrow(_)
        | ValueType::Stream(_)
        | ValueType::Future(_) => {
            return Cow::Borrowed(I32);
        }
        ValueType::Flags(flags) => {
            return Cow::Owned(vec![FlatType::I32; flags_chunk_count(flags)]);
        }
        ValueType::Record(record) => record.abi_shape(),
        ValueType::Tuple(tuple) => tuple.abi_shape(),
        ValueType::Variant(variant) => variant.abi_shape(),
        ValueType::Option(option) => option.abi_shape(),
        ValueType::Result(result) => result.abi_shape(),
        ValueType::FixedLengthList(fixed) => fixed.abi_shape(),
    };
    if let Some(flat) = shape.flat() {
        return Cow::Borrowed(flat);
    }
    Cow::Owned(unbounded_flat_types(ty))
}

/// The flat slots of a type too large for the flat form, which no
/// shape caches. Only a caller that ignores [`flat_count`] asks.
fn unbounded_flat_types(ty: &ValueType) -> Vec<FlatType> {
    match ty {
        ValueType::Record(record) => record
            .fields()
            .iter()
            .flat_map(|field| flat_types(field.ty()).into_owned())
            .collect(),
        ValueType::Tuple(tuple) => tuple
            .elements()
            .iter()
            .flat_map(|element| flat_types(element).into_owned())
            .collect(),
        ValueType::Variant(variant) => {
            flat_types_variant(variant.cases().iter().map(|case| case.payload()))
        }
        ValueType::Option(option) => flat_types_variant([None, Some(option.payload())].into_iter()),
        ValueType::Result(result) => flat_types_variant([result.ok(), result.err()].into_iter()),
        ValueType::FixedLengthList(fixed) => {
            let element = flat_types(fixed.element());
            let mut out = Vec::with_capacity(element.len() * fixed.length() as usize);
            for _ in 0..fixed.length() {
                out.extend_from_slice(&element);
            }
            out
        }
        other => flat_types(other).into_owned(),
    }
}

fn flat_type_of_primitive(prim: PrimitiveType) -> FlatType {
    match prim {
        PrimitiveType::Bool
        | PrimitiveType::S8
        | PrimitiveType::U8
        | PrimitiveType::S16
        | PrimitiveType::U16
        | PrimitiveType::S32
        | PrimitiveType::U32
        | PrimitiveType::Char => FlatType::I32,
        PrimitiveType::S64 | PrimitiveType::U64 => FlatType::I64,
        PrimitiveType::F32 => FlatType::F32,
        PrimitiveType::F64 => FlatType::F64,
        // `string` flattens to two slots and is intercepted by
        // `flat_types`'s top-level match. Returning I32 here is
        // dead-code-safe.
        PrimitiveType::String => FlatType::I32,
    }
}

/// The flat-slot list for a discriminated union: one i32 for the
/// discriminant followed by the per-case payload flat slots, joined
/// position-wise with [`join_flat`].
fn flat_types_variant<'a, I>(payloads: I) -> Vec<FlatType>
where
    I: Iterator<Item = Option<&'a ValueType>>,
{
    let mut out: Vec<FlatType> = vec![FlatType::I32];
    for payload in payloads.flatten() {
        for (i, slot) in flat_types(payload).iter().copied().enumerate() {
            match out.get_mut(i + 1) {
                Some(joined) => *joined = join_flat(*joined, slot),
                None => out.push(slot),
            }
        }
    }
    out
}

/// The canonical-ABI's `join` operation on flat slot types: when
/// two variant arms disagree, widen to the type that admits both.
/// Per the specification's `join`, `i32` and `f32` join to `i32`,
/// and every other disagreement joins to `i64`.
pub fn join_flat(a: FlatType, b: FlatType) -> FlatType {
    if a == b {
        return a;
    }
    match (a, b) {
        (FlatType::I32, FlatType::F32) | (FlatType::F32, FlatType::I32) => FlatType::I32,
        _ => FlatType::I64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FutureType, RecordField, RecordType, StreamType};

    #[wcmp_macros::test]
    fn it_lays_out_a_stream_and_a_future_as_a_handle() {
        // Whatever the payload, the value is the index of a readable
        // end: four bytes in memory and one `i32` slot.
        let string = || Some(ValueType::Primitive(PrimitiveType::String));
        for ty in [
            ValueType::Stream(StreamType::new(string())),
            ValueType::Stream(StreamType::new(None)),
            ValueType::Future(FutureType::new(string())),
            ValueType::Future(FutureType::new(None)),
        ] {
            assert_eq!(size_of(&ty), 4, "{ty:?}");
            assert_eq!(alignment_of(&ty), 4, "{ty:?}");
            assert_eq!(flat_types(&ty).as_ref(), &[FlatType::I32], "{ty:?}");
        }
    }

    #[wcmp_macros::test]
    fn it_lays_out_a_stream_inside_a_record_as_a_handle_field() {
        let record = ValueType::Record(RecordType::new([
            RecordField::new("tag", ValueType::Primitive(PrimitiveType::U8)),
            RecordField::new(
                "body",
                ValueType::Stream(StreamType::new(Some(ValueType::Primitive(
                    PrimitiveType::U8,
                )))),
            ),
        ]));
        assert_eq!(size_of(&record), 8);
        assert_eq!(
            flat_types(&record).as_ref(),
            &[FlatType::I32, FlatType::I32]
        );
    }
}
