// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A function type with what the canonical ABI derives from it once.
//!
//! Every call through an export or a lowered import asks the same
//! questions of the function's type: whether the parameter tuple
//! spills to memory, where each parameter sits in the spilled tuple,
//! whether the result comes back through a pointer, and how many core
//! result slots the core function returns. The answers are fixed by
//! the type, so [`Signature`] computes them when the component is
//! translated and a call reads them back rather than laying the
//! tuple out again.

use crate::component::FunctionType;
use crate::types::ValueType;

use super::layout::{
    MAX_FLAT_ASYNC_PARAMS, MAX_FLAT_PARAMS, SpillLayout, flat_param_count, flat_types,
    result_spills, spill_layout,
};

/// A component-level function type, with its canonical-ABI layout.
///
/// The layout is derived from the type and says nothing the type does
/// not, so a signature is shared behind an `Arc` by everything that
/// calls through it: the export's handle, the task record of each
/// call, and the trampoline of a lowered import.
#[derive(Debug)]
pub struct Signature {
    /// The function type as the component declares it.
    ty: FunctionType,
    /// The flat-slot count of the parameter tuple, or `None` when one
    /// parameter alone is too wide for flat slots.
    flat_param_count: Option<usize>,
    /// Where each parameter sits in the spilled parameter tuple, and
    /// the tuple's size and alignment. Computed whether or not the
    /// tuple spills, because the two lowerings measure the tuple
    /// against different limits.
    parameter_layout: SpillLayout,
    /// Whether the result travels through a pointer into memory.
    result_spills: bool,
    /// The number of core result slots a synchronous lift returns.
    core_result_arity: usize,
    /// The indices of the parameters whose type holds an
    /// `error-context` at any depth, so a host value of another store
    /// can reach the call through them.
    error_context_params: Vec<usize>,
}

impl Signature {
    /// Derive the layout of `ty`.
    pub fn new(ty: FunctionType) -> Self {
        let types: Vec<ValueType> = ty.parameters.iter().map(|p| p.ty.clone()).collect();
        let parameter_layout = spill_layout(&types);
        let result_spills = result_spills(&ty);
        let core_result_arity = match &ty.result {
            None => 0,
            Some(_) if result_spills => 1,
            Some(result_ty) => flat_types(result_ty).len(),
        };
        Self {
            flat_param_count: flat_param_count(&ty),
            error_context_params: types
                .iter()
                .enumerate()
                .filter(|(_, ty)| holds_error_context(ty))
                .map(|(index, _)| index)
                .collect(),
            ty,
            parameter_layout,
            result_spills,
            core_result_arity,
        }
    }

    /// The function type as the component declares it.
    pub fn ty(&self) -> &FunctionType {
        &self.ty
    }

    /// Whether a synchronous crossing passes the parameter tuple
    /// through one pointer rather than in flat slots.
    pub fn params_spill(&self) -> bool {
        !matches!(self.flat_param_count, Some(n) if n <= MAX_FLAT_PARAMS)
    }

    /// Whether an asynchronous lower passes the parameter tuple
    /// through one pointer rather than in flat slots. The limit is
    /// lower than a synchronous crossing's.
    pub fn async_params_spill(&self) -> bool {
        !matches!(self.flat_param_count, Some(n) if n <= MAX_FLAT_ASYNC_PARAMS)
    }

    /// Whether a parameter's type holds an `error-context` at any
    /// depth. A call checks the store of its arguments' error contexts
    /// only then.
    pub fn params_hold_error_context(&self) -> bool {
        !self.error_context_params.is_empty()
    }

    /// The indices of the parameters whose type holds an
    /// `error-context` at any depth, in order. An untyped call walks
    /// only these arguments for another store's error context, so a
    /// `list<u8>` beside one is not walked.
    pub fn error_context_params(&self) -> &[usize] {
        &self.error_context_params
    }

    /// The layout of the parameter tuple when it spills to memory.
    pub fn parameter_layout(&self) -> &SpillLayout {
        &self.parameter_layout
    }

    /// Whether the result travels through a pointer into memory.
    pub fn result_spills(&self) -> bool {
        self.result_spills
    }

    /// The number of core result slots a synchronous lift of this
    /// function returns: none without a result, one pointer for a
    /// spilled result, and the flat slots otherwise.
    pub fn core_result_arity(&self) -> usize {
        self.core_result_arity
    }
}

/// Whether a value of type `ty` can hold an error context. A stream's or
/// a future's payload does not count: the value is a readable end, which
/// carries no error context of its own.
fn holds_error_context(ty: &ValueType) -> bool {
    match ty {
        ValueType::ErrorContext => true,
        ValueType::Record(record) => record
            .fields()
            .iter()
            .any(|field| holds_error_context(field.ty())),
        ValueType::Variant(variant) => variant
            .cases()
            .iter()
            .any(|case| case.payload().is_some_and(holds_error_context)),
        ValueType::List(list) => holds_error_context(list.element()),
        ValueType::Map(map) => holds_error_context(map.key()) || holds_error_context(map.value()),
        ValueType::FixedLengthList(list) => holds_error_context(list.element()),
        ValueType::Option(option) => holds_error_context(option.payload()),
        ValueType::Result(result) => {
            result.ok().is_some_and(holds_error_context)
                || result.err().is_some_and(holds_error_context)
        }
        ValueType::Tuple(tuple) => tuple.elements().iter().any(holds_error_context),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::FunctionParameter;
    use crate::types::PrimitiveType;

    fn function(parameters: usize, result: Option<ValueType>) -> FunctionType {
        FunctionType {
            parameters: (0..parameters)
                .map(|i| FunctionParameter {
                    name: format!("p{i}"),
                    ty: ValueType::Primitive(PrimitiveType::U32),
                })
                .collect(),
            result,
            async_: false,
        }
    }

    #[wcmp_macros::test]
    fn it_lays_out_the_parameter_tuple_once_for_both_limits() {
        let signature = Signature::new(function(5, None));
        assert!(!signature.params_spill(), "five slots fit the sync limit");
        assert!(
            signature.async_params_spill(),
            "five slots exceed the async limit"
        );
        let layout = signature.parameter_layout();
        assert_eq!(layout.offsets, vec![0, 4, 8, 12, 16]);
        assert_eq!((layout.size, layout.alignment), (20, 4));
    }

    #[wcmp_macros::test]
    fn it_counts_the_core_result_slots_of_each_result_shape() {
        let string = ValueType::Primitive(PrimitiveType::String);
        let u32 = ValueType::Primitive(PrimitiveType::U32);
        assert_eq!(Signature::new(function(0, None)).core_result_arity(), 0);
        assert_eq!(
            Signature::new(function(0, Some(u32))).core_result_arity(),
            1
        );
        let spilled = Signature::new(function(0, Some(string)));
        assert!(spilled.result_spills());
        assert_eq!(spilled.core_result_arity(), 1);
    }

    #[wcmp_macros::test]
    fn it_marks_parameters_that_hold_an_error_context_at_any_depth() {
        use crate::types::{
            FixedLengthListType, ListType, MapType, OptionType, RecordField, RecordType,
            ResultType, StreamType, TupleType, VariantCase, VariantType,
        };
        let with = |ty: ValueType| {
            Signature::new(FunctionType {
                parameters: vec![FunctionParameter {
                    name: "p".to_owned(),
                    ty,
                }],
                result: None,
                async_: false,
            })
            .params_hold_error_context()
        };
        assert!(!Signature::new(function(3, None)).params_hold_error_context());
        assert!(with(ValueType::ErrorContext));
        assert!(with(ValueType::List(ListType::new(ValueType::Option(
            OptionType::new(ValueType::ErrorContext)
        )))));
        assert!(with(ValueType::Record(RecordType::new([
            RecordField::new("e", ValueType::ErrorContext)
        ]))));
        let u32 = || ValueType::Primitive(PrimitiveType::U32);
        let error = || ValueType::ErrorContext;
        for (shape, ty) in [
            (
                "variant",
                ValueType::Variant(VariantType::new([
                    VariantCase::new("none", None),
                    VariantCase::new("failed", Some(error())),
                ])),
            ),
            ("map key", ValueType::Map(MapType::new(error(), u32()))),
            ("map value", ValueType::Map(MapType::new(u32(), error()))),
            (
                "fixed-length list",
                ValueType::FixedLengthList(FixedLengthListType::new(error(), 2)),
            ),
            ("option", ValueType::Option(OptionType::new(error()))),
            (
                "result ok",
                ValueType::Result(ResultType::new(Some(error()), None)),
            ),
            (
                "result err",
                ValueType::Result(ResultType::new(Some(u32()), Some(error()))),
            ),
            ("tuple", ValueType::Tuple(TupleType::new([u32(), error()]))),
        ] {
            assert!(with(ty), "{shape}");
        }
        assert!(
            !with(ValueType::Variant(VariantType::new([VariantCase::new(
                "count",
                Some(u32())
            )]))),
            "a variant of no error context"
        );
        assert!(
            !with(ValueType::Stream(StreamType::new(Some(
                ValueType::ErrorContext
            )))),
            "a readable end carries no error context of its own"
        );
        assert!(!with(ValueType::List(ListType::new(ValueType::Primitive(
            PrimitiveType::U8
        )))));
    }
}
