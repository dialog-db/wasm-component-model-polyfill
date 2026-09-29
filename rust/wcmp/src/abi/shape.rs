//! The canonical-ABI shape a compound value type carries with it.
//!
//! A value type's size, alignment, and flat slots are fixed the
//! moment the type is built, and every crossing of a value of that
//! type needs them, most of all a list, which needs them once per
//! element. A compound type therefore computes its [`AbiShape`] in
//! its constructor, from the shapes its children already carry, and
//! a crossing reads it back without recomputing or allocating. A
//! function type holds its parameter and result types, so the shape
//! of every value a call moves is computed once, when the function
//! type is built.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use wasmtime_environ::component::{CanonicalAbiInfo, FlatType, VariantInfo};

use crate::abi::layout::{canonical_abi, flat_types, join_flat};
use crate::types::ValueType;

/// The canonical-ABI size, alignment, flat-slot count, and flat slots
/// of one compound value type.
///
/// The shape is derived from the type's structure and says nothing
/// the structure does not, so two shapes always compare equal and
/// hash to nothing: a type's equality and hash stay its structure's.
#[derive(Clone)]
pub struct AbiShape {
    /// Size, alignment, and flat-slot count, as Wasmtime computes
    /// them.
    info: CanonicalAbiInfo,
    /// The flat slots the type flattens to, when it flattens at all.
    /// A type whose flat-slot count exceeds the canonical ABI's
    /// limit has none cached, because no crossing passes it in flat
    /// slots.
    flat: Option<Arc<[FlatType]>>,
}

impl AbiShape {
    /// The shape of a record or tuple whose fields, in order, are
    /// `fields`.
    pub fn record<'a>(fields: impl Iterator<Item = &'a ValueType> + Clone) -> Self {
        let infos: Vec<CanonicalAbiInfo> = fields.clone().map(canonical_abi).collect();
        let info = CanonicalAbiInfo::record(infos.iter());
        let flat = info.flat_count.map(|count| {
            let mut slots = Vec::with_capacity(usize::from(count));
            for field in fields {
                slots.extend_from_slice(&flat_types(field));
            }
            Arc::from(slots)
        });
        Self { info, flat }
    }

    /// The shape of a discriminated union whose cases carry the
    /// payloads `cases`, in order: a variant, an option, or a result.
    pub fn variant<'a>(
        cases: impl ExactSizeIterator<Item = Option<&'a ValueType>> + Clone,
    ) -> Self {
        let infos: Vec<Option<CanonicalAbiInfo>> =
            cases.clone().map(|case| case.map(canonical_abi)).collect();
        let (_, info) = VariantInfo::new(infos.iter().map(Option::as_ref));
        let flat = info.flat_count.map(|count| {
            let mut slots = Vec::with_capacity(usize::from(count));
            slots.push(FlatType::I32);
            for payload in cases.flatten() {
                for (i, slot) in flat_types(payload).iter().copied().enumerate() {
                    match slots.get_mut(i + 1) {
                        Some(joined) => *joined = join_flat(*joined, slot),
                        None => slots.push(slot),
                    }
                }
            }
            Arc::from(slots)
        });
        Self { info, flat }
    }

    /// The shape of a fixed-length list of `length` copies of
    /// `element`, which is laid out as a tuple of those copies.
    pub fn fixed_length_list(element: &ValueType, length: u32) -> Self {
        let element_info = canonical_abi(element);
        let info = CanonicalAbiInfo::record(std::iter::repeat_n(&element_info, length as usize));
        let flat = info.flat_count.map(|count| {
            let element_flat = flat_types(element);
            let mut slots = Vec::with_capacity(usize::from(count));
            for _ in 0..length {
                slots.extend_from_slice(&element_flat);
            }
            Arc::from(slots)
        });
        Self { info, flat }
    }

    /// Size, alignment, and flat-slot count.
    pub fn info(&self) -> &CanonicalAbiInfo {
        &self.info
    }

    /// The flat slots, when the type flattens within the limit.
    pub fn flat(&self) -> Option<&[FlatType]> {
        self.flat.as_deref()
    }
}

impl PartialEq for AbiShape {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for AbiShape {}

impl Hash for AbiShape {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

impl fmt::Debug for AbiShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AbiShape")
            .field("size", &self.info.size32)
            .field("alignment", &self.info.align32)
            .field("flat", &self.flat)
            .finish()
    }
}
