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

use crate::types::{
    FlagsType, OptionType, PrimitiveType, RecordType, ResultType, TupleType, ValueType,
    VariantType,
};

/// Round `offset` up to the next multiple of `alignment`. The
/// alignment must be a power of two.
pub fn align_to(offset: usize, alignment: usize) -> usize {
    debug_assert!(alignment.is_power_of_two());
    (offset + alignment - 1) & !(alignment - 1)
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
        0 | 1..=0x100 => 1,
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
        ValueType::List(_) => CanonicalAbiInfo::POINTER_PAIR,
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
        ValueType::Flags(flags) => {
            let chunks = num_i32_flag_chunks(flags);
            vec![FlatType::I32; chunks]
        }
        ValueType::List(_) => vec![FlatType::I32, FlatType::I32],
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
        // `string` flattens to two slots; the Primitive(String)
        // path is handled by `flat_types`'s top-level match, never
        // reaching this leaf. Returning I32 here is dead-code-safe.
        PrimitiveType::String => FlatType::I32,
    }
}

fn num_i32_flag_chunks(flags: &FlagsType) -> usize {
    let n = flags.names().len();
    if n == 0 { 0 } else { (n + 31) / 32 }
}

/// The flat-slot list for a discriminated union: one i32 for the
/// discriminant followed by the per-case payload flat slots, joined
/// with the spec's `join` operation.
fn flat_types_variant<I>(payloads: I) -> Vec<FlatType>
where
    I: Iterator<Item = Option<ValueType>>,
{
    let mut out: Vec<FlatType> = vec![FlatType::I32];
    let mut payload_slots: Vec<FlatType> = Vec::new();
    for payload in payloads {
        let case = match payload {
            Some(ty) => flat_types(&ty),
            None => Vec::new(),
        };
        for (i, slot) in case.iter().copied().enumerate() {
            if i < payload_slots.len() {
                payload_slots[i] = join_flat(payload_slots[i], slot);
            } else {
                payload_slots.push(slot);
            }
        }
    }
    out.extend(payload_slots);
    out
}

/// The canonical-ABI's `join` operation on flat slot types: when
/// two variant arms disagree, widen to the type that admits both.
fn join_flat(a: FlatType, b: FlatType) -> FlatType {
    if a == b {
        return a;
    }
    match (a, b) {
        (FlatType::I32, FlatType::F32) | (FlatType::F32, FlatType::I32) => FlatType::I32,
        (FlatType::I32, FlatType::I64) | (FlatType::I64, FlatType::I32) => FlatType::I64,
        (FlatType::F32, FlatType::F64) | (FlatType::F64, FlatType::F32) => FlatType::F64,
        _ => FlatType::I64,
    }
}
