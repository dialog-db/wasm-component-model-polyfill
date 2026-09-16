//! Canonical-ABI layout: size, alignment, and flat-slot count.
//!
//! Size and alignment are computed via
//! [`wasmtime_environ::component::CanonicalAbiInfo`], the
//! spec-conformant constructors wasmtime exposes for the per-shape
//! arithmetic. The [`flat_types`] function — which produces the
//! ordered list of [`FlatType`] slots a value lowers to — is
//! recursive over [`ValueType`] and is the only piece wasmtime-
//! environ does not expose at function granularity (its public
//! flat-types accessor is a method on the wasmtime types builder
//! that takes an `InterfaceType`, a type the polyfill does not
//! project from `ValueType`).

use wasmtime_environ::component::CanonicalAbiInfo;

pub use wasmtime_environ::component::FlatType;

use crate::component::FunctionType;
use crate::types::{
    FlagsType, OptionType, PrimitiveType, RecordType, ResultType, TupleType, ValueType, VariantType,
};

/// The largest number of flat core-Wasm parameter slots a call
/// passes directly. Beyond this the canonical ABI spills the whole
/// parameter tuple into linear memory and passes one `i32` pointer.
pub const MAX_FLAT_PARAMS: usize = 16;

/// The largest number of flat core-Wasm result slots a call returns
/// directly. Beyond this the canonical ABI returns the result
/// through a pointer into linear memory.
pub const MAX_FLAT_RESULTS: usize = 1;

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

/// Build a [`CanonicalAbiInfo`] for a polyfill [`ValueType`] by
/// projecting recursively onto the wasmtime constructors.
fn canonical_abi(ty: &ValueType) -> CanonicalAbiInfo {
    match ty {
        ValueType::Primitive(prim) => primitive_abi(*prim),
        ValueType::Record(record) => record_abi(record),
        ValueType::Tuple(tuple) => tuple_abi(tuple),
        ValueType::Variant(variant) => variant_abi(variant),
        ValueType::Option(option) => option_abi(option),
        ValueType::Result(result) => result_abi(result),
        ValueType::Enum(en) => CanonicalAbiInfo::enum_(en.cases().len()),
        ValueType::Flags(flags) => CanonicalAbiInfo::flags(flags.names().len()),
        ValueType::List(_) | ValueType::Map(_) => CanonicalAbiInfo::POINTER_PAIR,
        // Laid out as a tuple of `N` copies of the element.
        ValueType::FixedLengthList(fixed) => {
            let element = canonical_abi(fixed.element());
            CanonicalAbiInfo::record(std::iter::repeat_n(&element, fixed.length() as usize))
        }
        ValueType::Own(_) | ValueType::Borrow(_) => CanonicalAbiInfo::SCALAR4,
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

fn record_abi(record: &RecordType) -> CanonicalAbiInfo {
    let infos: Vec<CanonicalAbiInfo> = record
        .fields()
        .iter()
        .map(|f| canonical_abi(f.ty()))
        .collect();
    CanonicalAbiInfo::record(infos.iter())
}

fn tuple_abi(tuple: &TupleType) -> CanonicalAbiInfo {
    let infos: Vec<CanonicalAbiInfo> = tuple.elements().iter().map(canonical_abi).collect();
    CanonicalAbiInfo::record(infos.iter())
}

fn variant_abi(variant: &VariantType) -> CanonicalAbiInfo {
    let case_payloads: Vec<Option<CanonicalAbiInfo>> = variant
        .cases()
        .iter()
        .map(|case| case.payload().map(canonical_abi))
        .collect();
    canonical_abi_variant(case_payloads.iter().map(Option::as_ref))
}

fn option_abi(option: &OptionType) -> CanonicalAbiInfo {
    let payload = canonical_abi(option.payload());
    canonical_abi_variant([None, Some(&payload)].into_iter())
}

fn result_abi(result: &ResultType) -> CanonicalAbiInfo {
    let ok = result.ok().map(canonical_abi);
    let err = result.err().map(canonical_abi);
    canonical_abi_variant([ok.as_ref(), err.as_ref()].into_iter())
}

/// Wrapper around [`CanonicalAbiInfo::variant`] keyed off
/// `Option<&CanonicalAbiInfo>` rather than its raw iterator
/// signature, so the call sites read clearly. The wasmtime
/// constructor itself is private inside `CanonicalAbiInfo::variant`
/// — its signature is `pub fn variant<'a, I>(cases: I)` where the
/// item type is `Option<&'a CanonicalAbiInfo>`.
fn canonical_abi_variant<'a, I>(cases: I) -> CanonicalAbiInfo
where
    I: ExactSizeIterator<Item = Option<&'a CanonicalAbiInfo>>,
{
    let (_, abi) = wasmtime_environ::component::VariantInfo::new(cases);
    abi
}

/// The list of flat core-Wasm slots a value type flattens to. The
/// length matches [`flat_count`] when that returns `Some`; if the
/// type is too large to flatten, the list returned here is the full
/// per-slot list anyway, and callers test `flat_count(ty).is_some()`
/// to decide whether to pass via flat slots or via a single memory
/// pointer.
pub fn flat_types(ty: &ValueType) -> Vec<FlatType> {
    match ty {
        ValueType::Primitive(PrimitiveType::String) => vec![FlatType::I32, FlatType::I32],
        ValueType::Primitive(prim) => vec![flat_type_of_primitive(*prim)],
        ValueType::Record(record) => record
            .fields()
            .iter()
            .flat_map(|field| flat_types(field.ty()))
            .collect(),
        ValueType::Tuple(tuple) => tuple.elements().iter().flat_map(flat_types).collect(),
        ValueType::Variant(variant) => {
            flat_types_variant(variant.cases().iter().map(|case| case.payload().cloned()))
        }
        ValueType::Option(option) => {
            flat_types_variant([None, Some(option.payload().clone())].into_iter())
        }
        ValueType::Result(result) => {
            flat_types_variant([result.ok().cloned(), result.err().cloned()].into_iter())
        }
        ValueType::Enum(_) => vec![FlatType::I32],
        ValueType::Flags(flags) => vec![FlatType::I32; flags_chunk_count(flags)],
        ValueType::List(_) | ValueType::Map(_) => vec![FlatType::I32, FlatType::I32],
        ValueType::FixedLengthList(fixed) => {
            let element = flat_types(fixed.element());
            let mut out = Vec::with_capacity(element.len() * fixed.length() as usize);
            for _ in 0..fixed.length() {
                out.extend_from_slice(&element);
            }
            out
        }
        ValueType::Own(_) | ValueType::Borrow(_) => vec![FlatType::I32],
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
/// with the spec's `join` operation.
fn flat_types_variant<I>(payloads: I) -> Vec<FlatType>
where
    I: Iterator<Item = Option<ValueType>>,
{
    let mut out: Vec<FlatType> = vec![FlatType::I32];
    out.extend(join_flat_slots(payloads));
    out
}

/// The joined payload slots of a discriminated union: the per-case
/// payload flat slots combined position-wise with [`join_flat`].
/// The discriminant slot is not included.
pub fn join_flat_slots<I>(payloads: I) -> Vec<FlatType>
where
    I: Iterator<Item = Option<ValueType>>,
{
    let mut joined: Vec<FlatType> = Vec::new();
    for payload in payloads {
        let case = match payload {
            Some(ty) => flat_types(&ty),
            None => Vec::new(),
        };
        for (i, slot) in case.into_iter().enumerate() {
            if i < joined.len() {
                joined[i] = join_flat(joined[i], slot);
            } else {
                joined.push(slot);
            }
        }
    }
    joined
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
